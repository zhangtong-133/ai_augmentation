//! 显式固定合成基准；没有数据库、用户来源或授权状态。
use personal_ai_agent_core::feed_value_quality::{QualityCase, cases_for_suite};
use personal_ai_llm::{
    local::{LocalInference, LocalTarget},
    stream::IgnoreTextDeltas,
};

// Diagnostic labels only: the strict domain decoder remains the acceptance gate.
fn output_failure(raw: &str, count: usize, profile: &str) -> &'static str {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return "BENCH_FAILURE=output_json";
    };
    let Some(items) = value.get("items").and_then(serde_json::Value::as_array) else {
        return "BENCH_FAILURE=output_schema";
    };
    if value.as_object().is_none_or(|o| o.len() != 1) {
        return "BENCH_FAILURE=output_schema";
    }
    if items.len() != count {
        return "BENCH_FAILURE=output_count";
    }
    let mut ids = std::collections::BTreeSet::new();
    for item in items {
        if item.as_object().is_none_or(|o| {
            o.len() != 3
                || !o.contains_key("id")
                || !o.contains_key("score")
                || !o.contains_key("reason")
        }) {
            return "BENCH_FAILURE=output_schema";
        }
        if !item
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|id| id > 0 && id <= u64::try_from(count).unwrap_or(0) && ids.insert(id))
        {
            return "BENCH_FAILURE=output_ids";
        }
        if !item
            .get("score")
            .is_some_and(|v| v.is_null() || v.as_u64().is_some_and(|s| s <= 100))
        {
            return "BENCH_FAILURE=output_score";
        }
        if !item
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| {
                !s.trim().is_empty() && s.chars().count() <= 240 && !s.chars().any(char::is_control)
            })
        {
            return "BENCH_FAILURE=output_reason";
        }
    }
    if profile == "local-rss-v2"
        && items.iter().any(|item| {
            !personal_ai_agent_core::feed_value_local::LOCAL_REASONS
                .contains(&item["reason"].as_str().unwrap_or(""))
        })
    {
        return "BENCH_FAILURE=output_reason_category";
    }
    "BENCH_FAILURE=output_strict"
}

fn target(args: &[String]) -> Result<LocalTarget, &'static str> {
    if args.len() != 5 || args[0] != "case" || args[4] != "--use-local-benchmark" {
        return Err(
            "usage: local-value-benchmark manifest | preview CASE | case CASE ENDPOINT MODEL --use-local-benchmark",
        );
    }
    LocalTarget::new(&args[2], &args[3]).map_err(|_| "invalid local benchmark target")
}
fn select(id: &str, suite: &str, profile: &str) -> Result<QualityCase, &'static str> {
    cases_for_suite(suite, profile)
        .map_err(|_| "invalid benchmark corpus")?
        .into_iter()
        .find(|c| c.id() == id)
        .ok_or("unknown fixed benchmark case")
}
fn options(mut args: &[String]) -> Result<(&[String], &str, &str), &'static str> {
    let mut suite = None;
    let mut profile = None;
    while args.len() >= 2 {
        let slot = match args[args.len() - 2].as_str() {
            "--suite" => &mut suite,
            "--profile" => &mut profile,
            _ => break,
        };
        if slot.replace(args[args.len() - 1].as_str()).is_some() {
            return Err("duplicate benchmark option");
        }
        args = &args[..args.len() - 2];
    }
    Ok((
        args,
        suite.unwrap_or("baseline"),
        profile.unwrap_or(personal_ai_agent_core::feed_value_local::LOCAL_VALUE_PROFILE),
    ))
}
async fn run(args: &[String]) -> Result<(), &'static str> {
    let (args, suite, profile) = options(args)?;
    cases_for_suite(suite, profile).map_err(|_| "unknown benchmark suite/profile")?;
    if args == ["--help"] {
        println!(
            "local-value-benchmark manifest\nlocal-value-benchmark preview CASE\nlocal-value-benchmark case CASE ENDPOINT MODEL --use-local-benchmark\n可在命令末尾加 --suite baseline|challenge 和 --profile local-rss-v1|local-rss-v2。仅固定合成 RSS，无数据库；完整基准请用 make local-value-benchmark 复用显存保护。质量失败输出 quality_pass=false，组运行退出 2；传输/协议失败退出 1，无自动重试。"
        );
        return Ok(());
    }
    if args == ["manifest"] {
        let all = cases_for_suite(suite, profile)
            .map_err(|_| "invalid benchmark corpus")?
            .iter()
            .map(QualityCase::manifest)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "invalid benchmark manifest")?;
        println!(
            "{}",
            serde_json::to_string(&all).map_err(|_| "invalid benchmark manifest")?
        );
        return Ok(());
    }
    if args.len() == 2 && args[0] == "preview" {
        let case = select(&args[1], suite, profile)?;
        let request = case.request().map_err(|_| "invalid benchmark request")?;
        println!(
            "{}",
            serde_json::json!({"manifest":case.manifest().map_err(|_| "invalid benchmark manifest")?,
            "messages": request.messages.iter().map(|m| serde_json::json!({"role":format!("{:?}",m.role),"content":m.content})).collect::<Vec<_>>(),
            "max_output_tokens":request.max_output_tokens,"temperature":request.temperature})
        );
        return Ok(());
    }
    let target = target(args)?;
    let case = select(&args[1], suite, profile)?;
    let request = case.request().map_err(|_| "invalid benchmark request")?;
    let client =
        personal_ai_llm_local::LocalChatClient::new().map_err(|_| "local client unavailable")?;
    let started = std::time::Instant::now();
    let raw = client
        .infer(&target, &request, &IgnoreTextDeltas)
        .await
        .map_err(|_| "BENCH_FAILURE=transport")?;
    let result = case
        .evaluate(raw.as_bytes())
        .map_err(|_| output_failure(&raw, case.manifest().map_or(0, |m| m.items.len()), profile))?;
    println!(
        "{}",
        serde_json::json!({"result":result,"endpoint":target.endpoint(),"model":target.model(),
        "elapsed_ms":started.elapsed().as_millis(),"response_bytes":raw.len(),"synthetic_only":true})
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(message) = run(&std::env::args().skip(1).collect::<Vec<_>>()).await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_output_diagnostics_emit_only_fixed_codes() {
        assert_eq!(
            output_failure("private diagnostic must not escape", 2, "local-rss-v1"),
            "BENCH_FAILURE=output_json"
        );
        assert_eq!(
            output_failure("{\"items\":[]}", 2, "local-rss-v1"),
            "BENCH_FAILURE=output_count"
        );
        assert_eq!(
            output_failure(
                r#"{"items":[{"id":0,"score":0,"reason":"x"}]}"#,
                1,
                "local-rss-v1"
            ),
            "BENCH_FAILURE=output_ids"
        );
        assert_eq!(
            output_failure(
                r#"{"items":[{"id":1,"score":0,"reason":""}]}"#,
                1,
                "local-rss-v1"
            ),
            "BENCH_FAILURE=output_reason"
        );
        assert_eq!(
            output_failure(
                r#"{"items":[{"id":1,"score":0,"reason":"x","extra":"secret"}]}"#,
                1,
                "local-rss-v1"
            ),
            "BENCH_FAILURE=output_schema"
        );
    }
    #[test]
    fn execution_requires_explicit_benchmark_flag_loopback_and_fixed_case() {
        let mut args = vec![
            "case".into(),
            "rust_preference".into(),
            "http://127.0.0.1:11435".into(),
            "qwen3:4b".into(),
            "--use-local-benchmark".into(),
        ];
        assert!(target(&args).is_ok());
        assert!(select(&args[1], "baseline", "local-rss-v1").is_ok());
        args[4] = "--use-local".into();
        assert!(target(&args).is_err());
        args[4] = "--use-local-benchmark".into();
        args[2] = "https://remote.example".into();
        assert!(target(&args).is_err());
        assert!(select("user-request", "baseline", "local-rss-v1").is_err());
    }
}
