use personal_ai_domain::UserId;
use personal_ai_storage::{
    reply_budgets::ReplyDispatchStore,
    reply_operations::{
        ReplyConfigurationRecord, ReplyOperationsStore, validate_reply_audit_scope,
        validate_reply_revision,
    },
};
use personal_ai_storage_postgres::PostgresStore;
use serde_json::{Value, json};
use std::process::ExitCode;

const USAGE: &str = "usage: reply-operations configurations [--after REVISION] | configuration REVISION | disable REVISION [--apply] | ledger --user UUID --day YYYY-MM-DD --currency USD [--after CONVERSATION_UUID/REQUEST_UUID]";

#[derive(Debug, PartialEq)]
enum Command {
    Configurations(Option<String>),
    Configuration(String),
    Disable {
        revision: String,
        apply: bool,
    },
    Ledger {
        user: String,
        day: String,
        currency: String,
        after: Option<String>,
    },
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
    let command = parse(args)?;
    // 只读取连接配置，不执行迁移、不组装模型、缓存或后台执行器。
    let url =
        std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required and must be UTF-8")?;
    let store = PostgresStore::connect_existing(&url).await?;
    let (report, consistent) = match command {
        Command::Configurations(after) => {
            let page = store.list_reply_configurations(after.as_deref()).await?;
            (
                json!({"items": page.items.into_iter().map(configuration_json).collect::<Vec<_>>(), "next_cursor": page.next_cursor}),
                true,
            )
        }
        Command::Configuration(revision) => (
            configuration_json(store.get_reply_configuration(&revision).await?),
            true,
        ),
        Command::Disable { revision, apply } => {
            // 预览不存在的版本也返回错误；只有显式 --apply 才写入持久化停用记录。
            let mut record = store.get_reply_configuration(&revision).await?;
            if apply {
                store.disable_reply_configuration(&revision).await?;
                record = store.get_reply_configuration(&revision).await?;
            }
            (
                json!({"apply": apply, "configuration": configuration_json(record)}),
                true,
            )
        }
        Command::Ledger {
            user,
            day,
            currency,
            after,
        } => {
            let report = store
                .audit_reply_money(&UserId::new(user), &day, &currency, after.as_deref())
                .await?;
            let consistent = report.consistent;
            (serde_json::to_value(report)?, consistent)
        }
    };
    println!("{}", serde_json::to_string(&report)?);
    Ok(if consistent {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

fn configuration_json(record: ReplyConfigurationRecord) -> Value {
    let budget = record.budget;
    json!({
        "revision": record.configuration.revision, "model": record.configuration.model,
        "active": record.active,
        "valid_until_unix_ms": record.valid_until_unix_ms.to_string(),
        "created_at_unix_ms": record.created_at_unix_ms.to_string(),
        "disabled_at_unix_ms": record.disabled_at_unix_ms.map(|v| v.to_string()),
        "budget": {
            "currency": budget.currency, "provider": budget.provider,
            "price_version": budget.price_version, "counter_version": budget.counter_version,
            "input_price_micro_per_million": budget.input_price_per_million.to_string(),
            "output_price_micro_per_million": budget.output_price_per_million.to_string(),
            "input_token_bound": budget.input_token_bound.to_string(),
            "output_token_bound": budget.output_token_bound.to_string(),
            "request_limit_micro": budget.request_limit.to_string(),
            "daily_limit_micro": budget.daily_limit.to_string(),
        }
    })
}

fn revision(value: &str) -> Result<String, &'static str> {
    validate_reply_revision(value).map_err(|_| USAGE)?;
    Ok(value.to_owned())
}

fn parse(args: &[String]) -> Result<Command, &'static str> {
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["configurations"] => Ok(Command::Configurations(None)),
        ["configurations", "--after", after] => Ok(Command::Configurations(Some(revision(after)?))),
        ["configuration", value] => Ok(Command::Configuration(revision(value)?)),
        ["disable", value] => Ok(Command::Disable {
            revision: revision(value)?,
            apply: false,
        }),
        ["disable", value, "--apply"] => Ok(Command::Disable {
            revision: revision(value)?,
            apply: true,
        }),
        ["ledger", options @ ..] => ledger(options),
        _ => Err(USAGE),
    }
}

fn ledger(options: &[&str]) -> Result<Command, &'static str> {
    let (mut user, mut day, mut currency, mut after) = (None, None, None, None);
    let (pairs, remainder) = options.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(USAGE);
    }
    for option in pairs {
        let target = match option[0] {
            "--user" => &mut user,
            "--day" => &mut day,
            "--currency" => &mut currency,
            "--after" => &mut after,
            _ => return Err(USAGE),
        };
        if target.replace(option[1]).is_some() {
            return Err(USAGE);
        }
    }
    let user = uuid::Uuid::parse_str(user.ok_or(USAGE)?)
        .map_err(|_| USAGE)?
        .to_string();
    let day = day.ok_or(USAGE)?;
    let currency = currency.ok_or(USAGE)?;
    validate_reply_audit_scope(day, currency).map_err(|_| USAGE)?;
    if let Some(after) = after {
        let (conversation, request) = after.split_once('/').ok_or(USAGE)?;
        uuid::Uuid::parse_str(conversation).map_err(|_| USAGE)?;
        uuid::Uuid::parse_str(request).map_err(|_| USAGE)?;
    }
    Ok(Command::Ledger {
        user,
        day: day.to_owned(),
        currency: currency.to_owned(),
        after: after.map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_requires_explicit_apply_and_mutation_flags_are_rejected_for_queries() {
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            parse(&args("disable v1")).unwrap(),
            Command::Disable {
                revision: "v1".into(),
                apply: false
            }
        );
        assert_eq!(
            parse(&args("disable v1 --apply")).unwrap(),
            Command::Disable {
                revision: "v1".into(),
                apply: true
            }
        );
        for input in [
            "",
            "enable v1",
            "disable",
            "disable v1 --apply --apply",
            "configuration v1 --apply",
            "configurations --apply",
            "configurations --after",
            "disable v1 --typo",
        ] {
            assert!(parse(&args(input)).is_err());
        }
        for value in ["", "   ", "\0", &"v".repeat(129)] {
            assert!(parse(&["disable".into(), value.into()]).is_err());
        }
    }

    #[test]
    fn ledger_requires_an_explicit_owner_date_currency_and_valid_cursor() {
        let owner = uuid::Uuid::new_v4();
        let input = format!("ledger --user {owner} --day 2026-09-30 --currency USD");
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert!(parse(&args(&input)).is_ok());
        for suffix in [
            " --apply",
            " --currency EUR",
            " --after",
            " --after invalid",
            " --user invalid",
        ] {
            assert!(parse(&args(&format!("{input}{suffix}"))).is_err());
        }
        assert!(parse(&args(&input.replace("2026-09-30", "2026-02-29"))).is_err());
        assert!(parse(&args("ledger --day 2026-09-30 --currency USD")).is_err());
        assert!(parse(&args(&format!("{input} --after {owner}/{owner}"))).is_ok());
    }
}
