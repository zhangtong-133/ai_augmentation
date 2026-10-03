//! Explicit administrator binding. The database connection is not exposed to the web browser.
use super::store::Store;
use personal_ai_domain::UserId;
use personal_ai_llm_openai::chatgpt::{ChatGptClient, Error, Result};
use personal_ai_storage::subscription_connections::{
    SubscriptionConnectionStore, VerifiedSubscriptionConnection,
};
use personal_ai_storage_postgres::PostgresStore;
use uuid::Uuid;

pub fn is_command(args: &[String]) -> bool {
    matches!(
        args.get(1).map(String::as_str),
        Some("bind" | "connections" | "revoke-binding")
    )
}
struct Arguments {
    owner: UserId,
    connection: Option<String>,
    revision: i64,
}
fn uuid(value: &str) -> Result<String> {
    let parsed = Uuid::parse_str(value).map_err(|_| Error("invalid UUID"))?;
    if parsed.is_nil() {
        return Err(Error("nil UUID is not allowed"));
    }
    Ok(parsed.to_string())
}
fn parse(args: &[String]) -> Result<Arguments> {
    let invalid = || Error("invalid binding arguments; see --help");
    match args.get(1).map(String::as_str) {
        Some("bind") if args.len() == 6 => {
            super::store::check_label(&args[2])?;
            let revision = args[5].parse().map_err(|_| invalid())?;
            if !(0..998).contains(&revision) {
                return Err(invalid());
            }
            Ok(Arguments {
                owner: UserId::new(uuid(&args[3])?),
                connection: Some(uuid(&args[4])?),
                revision,
            })
        }
        Some("revoke-binding") if args.len() == 5 => {
            let revision = args[4].parse().map_err(|_| invalid())?;
            if !(1..1000).contains(&revision) {
                return Err(invalid());
            }
            Ok(Arguments {
                owner: UserId::new(uuid(&args[2])?),
                connection: Some(uuid(&args[3])?),
                revision,
            })
        }
        Some("connections") if matches!(args.len(), 3 | 4) => Ok(Arguments {
            owner: UserId::new(uuid(&args[2])?),
            connection: args.get(3).map(|v| uuid(v)).transpose()?,
            revision: 0,
        }),
        _ => Err(invalid()),
    }
}
pub async fn run(args: &[String]) -> Result<()> {
    let parsed = parse(args)?;
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| Error("DATABASE_URL required for explicit application binding"))?;
    let db = PostgresStore::connect_existing(&url)
        .await
        .map_err(|_| Error("application database unavailable; run migrations first"))?;
    if args[1] == "connections" {
        let page = db
            .list_subscription_connections(&parsed.owner, parsed.connection.as_deref())
            .await
            .map_err(|_| Error("cannot list subscription bindings"))?;
        println!(
            "{}",
            serde_json::to_string(&page).map_err(|_| Error("cannot encode binding metadata"))?
        );
        return Ok(());
    }
    let connection = parsed
        .connection
        .as_deref()
        .ok_or(Error("connection UUID required"))?;
    let saved=if args[1]=="revoke-binding" {
        db.revoke_subscription_connection(&parsed.owner,connection,parsed.revision).await
    } else {
        let mut local=Store::open(std::path::Path::new(&args[0]))?;
        let client=ChatGptClient::new()?;
        let r=local.data.accounts.get_mut(&args[2]).ok_or(Error("unknown account label"))?;
        if !r.plan_enabled() { return Err(Error("subscription permission required before binding")); }
        if r.needs_refresh()? { client.refresh(r).await?; local.save()?; }
        let r=local.data.accounts.get(&args[2]).ok_or(Error("unknown account label"))?;
        let models=client.models(r).await?;
        let expiry=i64::try_from(r.access_expires_at()?).ok().and_then(|s|s.checked_mul(1000)).ok_or(Error("invalid token expiry"))?;
        let time=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|Error("invalid system time"))?;
        let time=i64::try_from(time.as_millis()).map_err(|_|Error("invalid system time"))?;
        let input=VerifiedSubscriptionConnection {
            host_id:local.data.host_id.clone(),client_id:r.client_id.clone(),subject:r.subject.clone().ok_or(Error("verified account identity required"))?,
            label:args[2].clone(),models:models.into_iter().map(|m|m.slug).collect(),valid_until_unix_ms:expiry.min(time+3_500_000),
        };
        db.save_subscription_connection(&parsed.owner,connection,parsed.revision,&input).await
    }.map_err(|_|Error("binding not saved: check owner, identity, revision, expiry and quota; list bindings before retrying"))?;
    println!(
        "{}",
        serde_json::to_string(&saved).map_err(|_| Error("cannot encode binding metadata"))?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::parse;
    #[test]
    fn mutations_require_explicit_owner_connection_and_revision() {
        let id = uuid::Uuid::new_v4();
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert!(parse(&args(&format!("dir bind personal {id} {id} 0"))).is_ok());
        assert!(parse(&args(&format!("dir revoke-binding {id} {id} 1"))).is_ok());
        assert!(parse(&args(&format!("dir connections {id}"))).is_ok());
        for s in [
            format!("dir bind personal {id} {id}"),
            format!("dir bind personal {id} {id} -1"),
            format!("dir revoke-binding {id} {id} 0"),
            format!("dir bind personal invalid {id} 0"),
            format!("dir bind ../other {id} {id} 0"),
            format!("dir connections {id} invalid"),
        ] {
            assert!(parse(&args(&s)).is_err());
        }
    }
}
