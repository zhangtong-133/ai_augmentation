use personal_ai_domain::UserId;
use personal_ai_storage::learning_operations::LearningOperationsStore;
use personal_ai_storage_postgres::PostgresStore;
use std::process::ExitCode;

const USAGE: &str = "usage: learning-operations audit --user UUID [--after PLAN_UUID]";
fn parse(args: &[String]) -> Result<(String, Option<String>), &'static str> {
    if args.first().map(String::as_str) != Some("audit") {
        return Err(USAGE);
    }
    let mut user = None;
    let mut after = None;
    let (pairs, remainder) = args[1..].as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(USAGE);
    }
    for [flag, value] in pairs {
        let target = match flag.as_str() {
            "--user" => &mut user,
            "--after" => &mut after,
            _ => return Err(USAGE),
        };
        let value = uuid::Uuid::parse_str(value).map_err(|_| USAGE)?;
        if value.is_nil() {
            return Err(USAGE);
        }
        let value = value.to_string();
        if target.replace(value).is_some() {
            return Err(USAGE);
        }
    }
    Ok((user.ok_or(USAGE)?, after))
}
#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
async fn run(args: &[String]) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let (user, after) = parse(args)?;
    let url =
        std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required and must be UTF-8")?;
    // 不执行迁移，不读取学习正文，不修复或修改记录。
    let store = PostgresStore::connect_existing(&url).await?;
    let report = store
        .audit_learning(&UserId::new(user), after.as_deref())
        .await?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(if report.consistent {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audit_requires_explicit_owner_and_rejects_mutation_or_duplicate_flags() {
        let id = uuid::Uuid::new_v4();
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert!(parse(&args(&format!("audit --user {id} --after {id}"))).is_ok());
        for input in [
            String::new(),
            "audit".into(),
            "audit --user bad".into(),
            format!("audit --user {id} --apply"),
            format!("audit --user {id} --user {id}"),
            format!("audit --user {id} --after bad"),
            format!("audit --all {id}"),
            format!("audit --user {}", uuid::Uuid::nil()),
        ] {
            assert!(parse(&args(&input)).is_err());
        }
    }
}
