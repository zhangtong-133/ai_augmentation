use personal_ai_domain::UserId;
use personal_ai_knowledge::vector_maintenance::collect_vectors;
use personal_ai_storage_postgres::PostgresStore;
use personal_ai_storage_qdrant::QdrantStore;
use std::process::ExitCode;

const USAGE: &str = "usage: vector-maintenance gc --user UUID [--after POINT_UUID] [--apply --exclusive-collection --writers-stopped]";
struct Options {
    user: String,
    after: Option<String>,
    apply: bool,
}
fn parse(args: &[String]) -> Result<Options, &'static str> {
    if args.first().map(String::as_str) != Some("gc") {
        return Err(USAGE);
    }
    let (mut user, mut after, mut apply, mut exclusive, mut stopped) =
        (None, None, false, false, false);
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--user" | "--after" => {
                let id = uuid::Uuid::parse_str(args.next().ok_or(USAGE)?).map_err(|_| USAGE)?;
                if id.is_nil() {
                    return Err(USAGE);
                }
                let target = if arg == "--user" {
                    &mut user
                } else {
                    &mut after
                };
                if target.replace(id.to_string()).is_some() {
                    return Err(USAGE);
                }
            }
            "--apply" if !apply => apply = true,
            "--exclusive-collection" if !exclusive => exclusive = true,
            "--writers-stopped" if !stopped => stopped = true,
            _ => return Err(USAGE),
        }
    }
    if apply && !(exclusive && stopped) {
        return Err(
            "apply requires --exclusive-collection --writers-stopped: verify database ownership and stop/drain all index/import writers before cleanup",
        );
    }
    Ok(Options {
        user: user.ok_or(USAGE)?,
        after,
        apply,
    })
}
fn required(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(name).map_err(|_| format!("{name} is required and must be UTF-8").into())
}
async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse(args)?;
    let store = PostgresStore::connect_existing(&required("DATABASE_URL")?).await?;
    let key = match std::env::var("QDRANT_API_KEY") {
        Ok(s) => Some(s),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err("QDRANT_API_KEY must be UTF-8".into()),
    };
    let dimensions = required("EMBEDDING_DIMENSIONS")?
        .parse()
        .map_err(|_| "invalid EMBEDDING_DIMENSIONS")?;
    let vectors = QdrantStore::new(
        &required("QDRANT_URL")?,
        &required("QDRANT_COLLECTION")?,
        key.as_deref(),
        &required("OPENAI_EMBEDDING_MODEL")?,
        dimensions,
    )?;
    let report = collect_vectors(
        &store,
        &vectors,
        &UserId::new(options.user),
        options.after.as_deref(),
        options.apply,
    )
    .await?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_requires_owner_and_explicit_exclusive_drained_writers() {
        let id = uuid::Uuid::new_v4();
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert!(!parse(&args(&format!("gc --user {id}"))).unwrap().apply);
        assert!(
            parse(&args(&format!(
                "gc --user {id} --apply --exclusive-collection --writers-stopped"
            )))
            .unwrap()
            .apply
        );
        for s in [
            "gc".into(),
            format!("gc --user {id} --apply"),
            format!("gc --user {id} --apply --exclusive-collection"),
            format!("gc --user {id} --after bad"),
            format!("gc --user {id} --user {id}"),
            format!("gc --user {id} --unknown"),
        ] {
            assert!(parse(&args(&s)).is_err());
        }
    }
}
