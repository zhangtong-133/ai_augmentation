use api_server::{AppState, Config, router};
use personal_ai_storage_postgres::PostgresStore;
use std::{error::Error, sync::Arc};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let config = Config::from_env().map_err(std::io::Error::other)?;
    let mut store = PostgresStore::connect(&config.database_url).await?;
    if let Some(objects) = api_server::object_storage_from_env()? {
        store = store.with_object_storage(objects);
    }
    let store = Arc::new(store);
    let message_cache = api_server::message_cache_from_env().map_err(std::io::Error::other)?;
    let cleanup_task = message_cache.as_ref().map(|cache| {
        let cache = cache.clone();
        let store = store.clone();
        tokio::spawn(async move {
            loop {
                api_server::reconcile_message_deletions(store.as_ref(), cache.as_ref()).await;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        })
    });
    let indexing = api_server::indexing_from_env(store.clone()).await?;
    let answering = api_server::answering_from_env(indexing.is_some())?;
    let replies = api_server::ReplyRuntime::from_env(store.clone())
        .await
        .map_err(std::io::Error::other)?;
    let model_agents = api_server::ModelAgentRuntime::from_env(store.clone())
        .await
        .map_err(std::io::Error::other)?;
    let feeds = api_server::FeedRuntime::from_env(store.clone()).map_err(std::io::Error::other)?;
    let web_search = match std::env::var("WEB_SEARCH_URL") {
        Ok(url) if url.is_empty() => None,
        Ok(url) => Some(Arc::new(personal_ai_web_search::WebSearch::new(&url)?)),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err("WEB_SEARCH_URL must be UTF-8".into()),
    };
    let git_tool = match std::env::var("GIT_REPOSITORIES_JSON") {
        Ok(config) => personal_ai_git_local::GitLogTool::from_json(&config)?.map(Arc::new),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err("GIT_REPOSITORIES_JSON must be UTF-8".into()),
    };
    let listener = tokio::net::TcpListener::bind(config.address).await?;
    let reply_task = {
        let replies = replies.clone();
        tokio::spawn(async move { replies.run().await })
    };
    let index_task = indexing.as_ref().map(|indexing| {
        let indexing = indexing.clone();
        let documents = store.clone();
        tokio::spawn(async move { indexing.run_jobs(documents).await })
    });
    tracing::info!(address = %config.address, "API ready");
    axum::serve(
        listener,
        router(AppState {
            learning: Some(store.clone()),
            learning_text: api_server::learning_text_from_env().map_err(std::io::Error::other)?,
            subscription_connections: Some(store.clone()),
            feed_values: Some(store.clone()),
            feeds: Some(feeds),
            schedules: Some(store.clone()),
            model_agents: Some(model_agents),
            agent_plans: Some(store.clone()),
            web_search,
            git_tool,
            tool_calls: Some(store.clone()),
            replies: Some(replies),
            messages: store.clone(),
            message_cache,
            conversations: store.clone(),
            memories: store.clone(),
            indexing,
            answering,
            answer_authorizations: Some(store.clone()),
            web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
            store: store.clone(),
            documents: store,
            api_token: Arc::from(config.api_token),
            auth: Arc::new(api_server::AuthConfig::new(
                std::env::var("SESSION_COOKIE_SECURE").as_deref() != Ok("false"),
            )),
        }),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    reply_task.abort();
    let _ = reply_task.await;
    if let Some(task) = index_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = cleanup_task {
        task.abort();
        let _ = task.await;
    }
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("API shutting down");
}
