#![forbid(unsafe_code)]

use personal_ai_agent_core::schedules::deliver_due_reminders;
use personal_ai_storage_postgres::PostgresStore;
use std::{env, process::ExitCode, time::Duration};

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
    let url = env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required and must be UTF-8")?;
    // 迁移由 API/部署流程执行；调度进程不初始化供应商或缓存。
    let store = PostgresStore::connect_existing(&url).await?;
    loop {
        let delay = match deliver_due_reminders(&store).await {
            Ok(count) => {
                println!("scheduler delivered {count} reminders");
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
