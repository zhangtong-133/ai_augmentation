//! 显式本地 RSS 执行，不读取订阅或 API 凭据。
use personal_ai_domain::UserId;
use personal_ai_llm::local::LocalTarget;
use personal_ai_storage::feed_value::{FeedValueStore, ValuePricing};
use personal_ai_storage_postgres::PostgresStore;
fn uuid(value: &str) -> Result<String, &'static str> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|v| !v.is_nil())
        .map(|v| v.to_string())
        .ok_or("invalid local scoring UUID")
}
fn parse(args: &[String]) -> Result<(UserId, String, Option<LocalTarget>), &'static str> {
    match args.first().map(String::as_str) {
        Some("show") if args.len() == 3 => {
            Ok((UserId::new(uuid(&args[1])?), uuid(&args[2])?, None))
        }
        Some("run") if args.len() == 6 && args[5] == "--use-local" => Ok((
            UserId::new(uuid(&args[1])?),
            uuid(&args[2])?,
            Some(LocalTarget::new(&args[3], &args[4]).map_err(|_| "invalid local scoring target")?),
        )),
        _ => Err(
            "usage: local-value show OWNER REQUEST | run OWNER REQUEST ENDPOINT MODEL --use-local",
        ),
    }
}
async fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        println!(
            "local-value show OWNER REQUEST\nlocal-value run OWNER REQUEST ENDPOINT MODEL --use-local\n建议使用 make local-value 的显存保护入口；未知结果不重发。"
        );
        return Ok(());
    }
    let (owner, request, target) = parse(args)?;
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL required")?;
    let store = PostgresStore::connect(&url)
        .await
        .map_err(|_| "local scoring database unavailable")?;
    let mut saved = store
        .get_feed_value(&owner, &request)
        .await
        .map_err(|_| "local scoring unavailable")?;
    if let Some(target) = &target {
        if !matches!(&saved.pricing,ValuePricing::Local{endpoint,model,..}if endpoint==target.endpoint()&&model==target.model())
        {
            return Err("local target differs from original scoring consent");
        }
        if saved.status == "authorized" {
            let client = personal_ai_llm_local::LocalChatClient::new()
                .map_err(|_| "local client unavailable")?;
            saved = match personal_ai_agent_core::feed_value_local::execute_local_value(
                &store, &client, target, &owner, &request,
            )
            .await
            .map_err(|_| "outcome unknown; inspect original scoring request; do not resend")?
            {
                Some(saved) => saved,
                None => store
                    .get_feed_value(&owner, &request)
                    .await
                    .map_err(|_| "original scoring outcome unavailable")?,
            };
        }
    }
    println!(
        "{}",
        serde_json::json!({"id":saved.request_id,"status":saved.status,"digest":saved.digest,"pricing":saved.pricing,"scores":saved.scores})
    );
    if target.is_some() && saved.status != "succeeded" {
        return Err("scoring not completed; inspect original request; no automatic retry");
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
    fn local_scoring_requires_explicit_flag_and_loopback_target() {
        let id = uuid::Uuid::new_v4().to_string();
        let mut args = vec![
            "run".into(),
            id.clone(),
            id,
            "http://127.0.0.1:11435".into(),
            "qwen3:4b".into(),
            "--use-local".into(),
        ];
        assert!(parse(&args).is_ok());
        args[5] = "--use-subscription".into();
        assert!(parse(&args).is_err());
        args[5] = "--use-local".into();
        args[3] = "https://remote.example".into();
        assert!(parse(&args).is_err());
    }
}
