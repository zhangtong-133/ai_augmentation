//! Explicit local execution, independent of OAuth/subscription/API credentials.
use personal_ai_agent_core::learning_model_execution::{
    local::observe_local_review, text_bridge::relay_progress,
};
use personal_ai_domain::UserId;
use personal_ai_llm::local::LocalTarget;
use personal_ai_storage::{learning::LearningStore, learning::review_text::ReviewTextBridge};
use personal_ai_storage_postgres::PostgresStore;
#[path = "local-review/probe.rs"]
mod probe;
fn uuid(value: &str) -> Result<String, &'static str> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|v| !v.is_nil())
        .map(|v| v.to_string())
        .ok_or("invalid local review UUID")
}
fn parse(args: &[String]) -> Result<(UserId, String, Option<LocalTarget>), &'static str> {
    match args.first().map(String::as_str) {
        Some("show") if args.len() == 3 => {
            Ok((UserId::new(uuid(&args[1])?), uuid(&args[2])?, None))
        }
        Some("run") if args.len() == 6 && args[5] == "--use-local" => Ok((
            UserId::new(uuid(&args[1])?),
            uuid(&args[2])?,
            Some(LocalTarget::new(&args[3], &args[4]).map_err(|_| "invalid local target")?),
        )),
        _ => Err(
            "usage: local-review show OWNER REQUEST | run OWNER REQUEST ENDPOINT MODEL --use-local",
        ),
    }
}
async fn run(args: &[String]) -> Result<(), &'static str> {
    if args.first().map(String::as_str) == Some("probe") {
        if args.len() != 4 || args[3] != "--use-local" {
            return Err("usage: local-review probe ENDPOINT MODEL --use-local");
        }
        return probe::run(
            &LocalTarget::new(&args[1], &args[2]).map_err(|_| "invalid local target")?,
        )
        .await;
    }
    if args == ["--help"] {
        println!(
            "local-review show OWNER REQUEST\nlocal-review run OWNER REQUEST ENDPOINT MODEL --use-local\nlocal-review probe ENDPOINT MODEL --use-local\n执行前核对游戏显存预算；原授权仅发送一次，未知结果不重发。\nprobe 仅发送固定测试材料，不读取用户记录。"
        );
        return Ok(());
    }
    let (owner, request, target) = parse(args)?;
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL required")?;
    let store = PostgresStore::connect(&url)
        .await
        .map_err(|_| "local review database unavailable")?;
    let mut item = store
        .get_model_authorization(&owner, &request)
        .await
        .map_err(|_| "local review unavailable")?;
    if let Some(target) = &target {
        if item.local_endpoint.as_deref() != Some(target.endpoint()) || item.model != target.model()
        {
            return Err("local target differs from original consent");
        }
        if item.status == "authorized" {
            let client = personal_ai_llm_local::LocalChatClient::new()
                .map_err(|_| "local client unavailable")?;
            let bridge = match std::env::var("LEARNING_TEXT_REDIS_URL") {
                Ok(url) if !url.is_empty() => Some(
                    personal_ai_storage_redis::RedisReviewText::new(&url)
                        .map_err(|_| "invalid local text bridge")?,
                ),
                Ok(_) | Err(std::env::VarError::NotPresent) => None,
                Err(_) => return Err("invalid local text bridge"),
            };
            let mut publisher = if let Some(bridge) = bridge {
                bridge.publisher(&owner, &request).await.ok()
            } else {
                None
            };
            let (receiver, execution) =
                observe_local_review(&store, &client, target, &owner, &request);
            let result = if let Some(publisher) = publisher.as_mut() {
                let (result, ()) = tokio::join!(
                    execution,
                    relay_progress(receiver, publisher.as_mut(), &owner, &request)
                );
                result
            } else {
                drop(receiver);
                execution.await
            };
            item = match result
                .map_err(|_| "outcome unknown; inspect original request; do not resend")?
            {
                Some(item) => item,
                None => store
                    .get_model_authorization(&owner, &request)
                    .await
                    .map_err(|_| "outcome unavailable")?,
            };
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&item).map_err(|_| "invalid local result")?
    );
    if target.is_some() && item.status != "succeeded" {
        return Err("review not completed; inspect original status; no automatic retry");
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(error) = run(&std::env::args().skip(1).collect::<Vec<_>>()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_requires_explicit_local_flag_and_exact_loopback_target() {
        let args: Vec<String> = [
            "run",
            &uuid::Uuid::new_v4().to_string(),
            &uuid::Uuid::new_v4().to_string(),
            "http://127.0.0.1:11435",
            "qwen3:4b",
            "--use-local",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert!(parse(&args).is_ok());
        let mut bad = args.clone();
        bad[5] = "--use-subscription".into();
        assert!(parse(&bad).is_err());
        bad[5] = "--use-local".into();
        bad[3] = "http://192.168.1.1:11435".into();
        assert!(parse(&bad).is_err());
    }
}
