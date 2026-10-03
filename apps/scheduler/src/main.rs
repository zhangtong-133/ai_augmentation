#![forbid(unsafe_code)]

use personal_ai_agent_core::schedules::deliver_due_reminders;
use personal_ai_storage::brief_schedules::BriefScheduleStore;
use personal_ai_storage_postgres::PostgresStore;
use std::{env, process::ExitCode, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> ExitCode {
    tokio::select! {
        result = run() => match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => { eprintln!("{error}"); ExitCode::FAILURE }
        },
        () = shutdown() => ExitCode::SUCCESS,
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mode = match env::var("SCHEDULER_MODE") {
        Ok(mode) => mode,
        Err(env::VarError::NotPresent) => "disabled".into(),
        Err(_) => return Err("invalid SCHEDULER_MODE".into()),
    };
    let forever = env::var("RUN_FOREVER").as_deref() == Ok("1");
    match mode.as_str() {
        "disabled" => {
            println!("scheduler disabled");
            if forever {
                std::future::pending::<()>().await;
            }
            return Ok(());
        }
        "local" => (),
        _ => return Err("SCHEDULER_MODE must be disabled or local".into()),
    }
    let rss_enabled = match env::var("RSS_SCHEDULES_ENABLED") {
        Err(env::VarError::NotPresent) => false,
        Ok(value) if value == "false" => false,
        Ok(value) if value == "true" => true,
        _ => return Err("RSS_SCHEDULES_ENABLED must be true or false".into()),
    };
    let url = env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required and must be UTF-8")?;
    // 迁移由 API/部署流程执行；RSS 传输仅在显式启用后构造。
    let store = Arc::new(PostgresStore::connect_existing(&url).await?);
    if rss_enabled {
        tokio::try_join!(
            local_loop(&store, forever),
            feed_loop(store.clone(), forever)
        )?;
        Ok(())
    } else {
        local_loop(&store, forever).await
    }
}

async fn local_loop(
    store: &PostgresStore,
    forever: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let delay = match tick(store).await {
            Ok((reminders, briefs)) => {
                println!(
                    "scheduler delivered {reminders} reminders; processed {briefs} brief schedules"
                );
                2
            }
            Err(error) if forever => {
                eprintln!("scheduler batch failed: {error}");
                5
            }
            Err(error) => return Err(error.into()),
        };
        if !forever {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

async fn feed_loop(
    store: Arc<PostgresStore>,
    forever: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut runner = scheduler::FeedScheduleRunner::new(
        store,
        Arc::new(personal_ai_feed_http::PublicFeedTransport),
    );
    loop {
        let delay = match runner.tick().await {
            Ok(result) => {
                println!(
                    "RSS schedules: recovered {}; completed {}; skipped or unknown {}",
                    result.recovered, result.completed, result.skipped_or_unknown
                );
                2
            }
            Err(_) if forever => {
                eprintln!("RSS schedule batch unavailable");
                5
            }
            Err(_) => return Err("RSS schedule batch unavailable".into()),
        };
        if !forever {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

async fn tick(store: &PostgresStore) -> Result<(u32, u32), personal_ai_storage::StorageError> {
    let reminders = deliver_due_reminders(store).await?;
    let mut briefs = 0;
    for _ in 0..100 {
        if !store.generate_due_brief().await? {
            break;
        }
        briefs += 1;
    }
    Ok((reminders, briefs))
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install termination handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => (),
            _ = terminate.recv() => (),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
