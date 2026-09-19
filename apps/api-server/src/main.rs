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
    let indexing = api_server::indexing_from_env().await?;
    let listener = tokio::net::TcpListener::bind(config.address).await?;
    tracing::info!(address = %config.address, "API ready");
    let worker = indexing.as_ref().map(|indexing| {
        tokio::spawn(api_server::run_index_jobs(
            indexing.clone(),
            store.clone(),
            store.clone(),
        ))
    });
    axum::serve(
        listener,
        router(AppState {
            index_jobs: Some(store.clone()),
            indexing,
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
    if let Some(worker) = worker {
        worker.abort();
        let _ = worker.await;
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
