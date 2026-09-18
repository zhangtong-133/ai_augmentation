use personal_ai_storage_postgres::PostgresStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, apply, after) = parse(&args)?;
    let objects =
        api_server::object_storage_from_env()?.ok_or("OBJECT_STORE_ENABLED=true required")?;
    let store = PostgresStore::connect(&std::env::var("DATABASE_URL")?)
        .await?
        .with_object_storage(objects);
    let report = if mode == "migrate" {
        store.migrate_originals(apply, 100).await?
    } else {
        store.collect_originals(apply, &after).await?
    };
    println!(
        "{}",
        serde_json::json!({"apply":apply,"scanned":report.scanned,"eligible":report.eligible,"changed":report.changed,"next_cursor":report.next_cursor})
    );
    Ok(())
}

fn parse(args: &[String]) -> Result<(&str, bool, String), &'static str> {
    let usage = "usage: object-maintenance migrate [--apply] | gc [--apply --exclusive-bucket] [--after KEY]";
    let Some(mode) = args
        .first()
        .filter(|s| matches!(s.as_str(), "migrate" | "gc"))
    else {
        return Err(usage);
    };
    let mut apply = false;
    let mut exclusive = false;
    let mut after = String::new();
    let mut options = args.iter().skip(1);
    while let Some(option) = options.next() {
        match option.as_str() {
            "--apply" if !apply => apply = true,
            "--exclusive-bucket" if mode == "gc" && !exclusive => exclusive = true,
            "--after" if mode == "gc" && after.is_empty() => {
                after.clone_from(options.next().ok_or(usage)?);
            }
            _ => return Err(usage),
        }
    }
    if mode == "gc" && apply && !exclusive {
        return Err(
            "gc --apply requires --exclusive-bucket: confirm this bucket belongs only to this database and all writers use the maintenance lock protocol",
        );
    }
    Ok((mode, apply, after))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn execution_is_explicit_and_cleanup_requires_bucket_confirmation() {
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        for input in ["migrate", "gc", "gc --after users/key"] {
            assert!(!parse(&args(input)).unwrap().1);
        }
        for input in [
            "",
            "unknown",
            "gc --apply",
            "gc --after",
            "migrate --after key",
            "migrate --apply --apply",
            "gc --typo",
        ] {
            assert!(parse(&args(input)).is_err());
        }
        assert!(parse(&args("migrate --apply")).unwrap().1);
        assert!(parse(&args("gc --apply --exclusive-bucket")).unwrap().1);
    }
}
