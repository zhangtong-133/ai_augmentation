//! Explicit local administrator workflow; requires `DATABASE_URL`, never exposed as a browser API.
use super::{store::Store, value_runtime::Runtime};
use personal_ai_agent_core::{
    feed_value::plan_value_scoring, feed_value_execution::execute_subscription_value,
};
use personal_ai_domain::UserId;
use personal_ai_llm_openai::chatgpt::{ChatGptClient, Error, Result};
use personal_ai_storage::{
    StorageResult,
    feed_value::{
        FeedValueStore, ValueApproval, ValuePricing, ValueQuotePlanner, ValueReview, ValueSnapshot,
    },
    subscription_connections::SubscriptionConnectionStore,
};
use personal_ai_storage_postgres::PostgresStore;
use std::sync::Arc;
use uuid::Uuid;

pub fn is_command(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a.starts_with("value-"))
}
enum Action {
    Preview { connection: String, model: String },
    Approve { digest: String },
    Run { label: String },
    Show,
    Cancel,
}
struct Arguments {
    owner: UserId,
    request: String,
    action: Action,
}
fn uuid(value: &str) -> Result<String> {
    let id = Uuid::parse_str(value).map_err(|_| Error("invalid UUID"))?;
    if id.is_nil() {
        return Err(Error("nil UUID is not allowed"));
    }
    Ok(id.to_string())
}
fn parse(args: &[String]) -> Result<Arguments> {
    let invalid = || Error("invalid scoring arguments; see --help");
    let (owner, request, action) = match args.get(1).map(String::as_str) {
        Some("value-preview") if args.len() == 6 => {
            if args[4].trim().is_empty()
                || args[4].len() > 128
                || args[4].chars().any(char::is_control)
            {
                return Err(invalid());
            }
            (
                &args[2],
                &args[5],
                Action::Preview {
                    connection: uuid(&args[3])?,
                    model: args[4].clone(),
                },
            )
        }
        Some("value-approve")
            if args.len() == 7
                && args[5] == "--share-content"
                && args[6] == "--use-subscription" =>
        {
            if args[4].len() != 64
                || !args[4]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid());
            }
            (
                &args[2],
                &args[3],
                Action::Approve {
                    digest: args[4].clone(),
                },
            )
        }
        Some("value-run") if args.len() == 6 && args[5] == "--use-subscription" => {
            super::store::check_label(&args[2])?;
            (
                &args[3],
                &args[4],
                Action::Run {
                    label: args[2].clone(),
                },
            )
        }
        Some("value-show") if args.len() == 4 => (&args[2], &args[3], Action::Show),
        Some("value-cancel") if args.len() == 4 => (&args[2], &args[3], Action::Cancel),
        _ => return Err(invalid()),
    };
    Ok(Arguments {
        owner: UserId::new(uuid(owner)?),
        request: uuid(request)?,
        action,
    })
}
struct Planner(ValuePricing);
impl ValueQuotePlanner for Planner {
    fn quote(&self, _: &UserId, _: &str, _: &ValueSnapshot) -> StorageResult<ValuePricing> {
        Ok(self.0.clone())
    }
}
fn display(owner: &UserId, saved: &ValueReview) -> Result<serde_json::Value> {
    let plan = saved
        .snapshot
        .as_ref()
        .map(|s| {
            plan_value_scoring(
                owner,
                &saved.request_id,
                s.day_start_unix_ms,
                s.as_of_unix_ms,
                &s.keywords,
                &s.candidates,
            )
        })
        .transpose()
        .map_err(|_| Error("invalid saved scoring plan"))?
        .flatten();
    let shared=plan.as_ref().map(|p| {
        let request=p.request();
        serde_json::json!({"instructions":request.messages[0].content,"input":request.messages[1].content})
    });
    let mapping = plan.as_ref().map(|p| {
        p.brief()
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| serde_json::json!({"id":i+1,"entry":item.entry}))
            .collect::<Vec<_>>()
    });
    Ok(
        serde_json::json!({"request_id":saved.request_id,"status":saved.status,"digest":saved.digest,"pricing":saved.pricing,
        "expires_at_unix_ms":saved.expires_at_unix_ms,"approved_at_unix_ms":saved.approved_at_unix_ms,
        "shared_content":shared,"local_candidate_mapping":mapping,"scores":saved.scores,
        "usage_notice":"订阅用量可能消耗额度或账户设置允许的 credits；本地超时和响应大小不构成供应商用量上限。批准不等于执行；只有 value-run 会发起模型请求。评分仅供参考，不更新日报。"}),
    )
}
pub async fn run(args: &[String]) -> Result<()> {
    let arguments = parse(args)?;
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| Error("DATABASE_URL required for local scoring"))?;
    let db = PostgresStore::connect_existing(&url)
        .await
        .map_err(|_| Error("application database unavailable; run migrations first"))?;
    let saved = manage(&db, &args[0], &arguments).await?;
    println!("{}", display(&arguments.owner, &saved)?);
    if matches!(arguments.action, Action::Run { .. }) && saved.status != "succeeded" {
        return Err(Error(
            "scoring not completed; inspect status with value-show; no automatic retry",
        ));
    }
    Ok(())
}
async fn manage(db: &PostgresStore, directory: &str, args: &Arguments) -> Result<ValueReview> {
    let failed = |_| {
        Error(
            "scoring operation not confirmed; use value-show to inspect the original request before continuing",
        )
    };
    match &args.action {
        Action::Preview { connection, model } => {
            let c = db
                .get_subscription_connection(&args.owner, connection)
                .await
                .map_err(failed)?;
            let pricing = ValuePricing::Subscription {
                provider: "chatgpt-plan".into(),
                model: model.clone(),
                configuration_version: c.configuration_version(),
                connection_id: c.id,
                valid_until_unix_ms: c.valid_until_unix_ms,
            };
            db.preview_feed_value(&args.owner, &args.request, Arc::new(Planner(pricing)))
                .await
                .map_err(failed)
        }
        Action::Approve { digest } => db
            .approve_feed_value(
                &args.owner,
                &args.request,
                &ValueApproval {
                    digest: digest.clone(),
                    currency: None,
                    amount: None,
                    acknowledge_sharing: true,
                    acknowledge_cost: false,
                    acknowledge_subscription_usage: true,
                },
            )
            .await
            .map_err(failed),
        Action::Show => db
            .get_feed_value(&args.owner, &args.request)
            .await
            .map_err(failed),
        Action::Cancel => db
            .cancel_feed_value(&args.owner, &args.request)
            .await
            .map_err(failed),
        Action::Run { label } => {
            let saved = db
                .get_feed_value(&args.owner, &args.request)
                .await
                .map_err(failed)?;
            if saved.status != "authorized" {
                return Ok(saved);
            }
            if !matches!(saved.pricing, ValuePricing::Subscription { .. }) {
                return Err(Error("API reviews cannot use subscription execution"));
            }
            let mut local = Store::open(std::path::Path::new(directory))?;
            let client = ChatGptClient::new()?;
            let registration = local
                .data
                .accounts
                .get_mut(label)
                .ok_or(Error("unknown account label"))?;
            if !registration.plan_enabled() {
                return Err(Error("subscription permission required"));
            }
            // Persist token rotation before taking a non-replayable dispatch claim. The file lock
            // remains held through model verification, send and result persistence.
            if registration.needs_refresh()? {
                client.refresh(registration).await?;
                local.save()?;
            }
            let runtime = Runtime {
                client: &client,
                store: &local,
                label,
            };
            eprintln!(
                "正在核对已批准的原评分请求；通过检查后将消耗所选账户的订阅额度。未知结果不会重发。"
            );
            match execute_subscription_value(db, &runtime, &args.owner, &args.request)
                .await
                .map_err(failed)?
            {
                Some(saved) => Ok(saved),
                None => db
                    .get_feed_value(&args.owner, &args.request)
                    .await
                    .map_err(failed),
            }
        }
    }
}

#[cfg(test)]
mod tests;
