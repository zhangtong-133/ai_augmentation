//! 调用者必须持有用户、对话锁，并与回复状态修改共用事务。
use crate::map_error;
use personal_ai_agent_core::budget::{
    BillableUsage, BudgetError, CostReservation, Settlement, TokenPrices,
};
use personal_ai_storage::{
    StorageError, StorageResult,
    replies::{ReplyConfiguration, ReplyContext},
    reply_budgets::{ReplyBudget, ReplyUsage},
};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

fn budget_error(error: BudgetError) -> StorageError {
    match error {
        BudgetError::RequestLimitExceeded | BudgetError::DailyLimitExceeded => {
            StorageError::Conflict("reply money quota reached".into())
        }
        _ => StorageError::InvalidData("invalid reply money budget".into()),
    }
}

fn quote(budget: &ReplyBudget) -> StorageResult<CostReservation> {
    CostReservation::quote(
        TokenPrices {
            input_per_million: budget.input_price_per_million,
            output_per_million: budget.output_price_per_million,
        },
        budget.input_token_bound,
        budget.output_token_bound,
        budget.request_limit,
    )
    .map_err(budget_error)
}

pub(super) fn validate(
    budget: ReplyBudget,
    context: &ReplyContext,
) -> StorageResult<(ReplyBudget, CostReservation)> {
    if budget.currency.len() != 3
        || !budget.currency.bytes().all(|b| b.is_ascii_uppercase())
        || [
            &budget.provider,
            &budget.price_version,
            &budget.counter_version,
        ]
        .iter()
        .any(|s| s.trim().is_empty() || s.len() > 128 || s.contains('\0'))
        || budget.output_token_bound != u64::from(context.max_output_tokens)
        || budget.daily_limit <= 0
    {
        return Err(StorageError::InvalidData(
            "invalid reply money configuration".into(),
        ));
    }
    let quote = quote(&budget)?;
    Ok((budget, quote))
}

pub(super) async fn reserve(
    tx: &mut PgConnection,
    owner: Uuid,
    ids: (Uuid, Uuid),
    day: &str,
    configuration: &ReplyConfiguration,
    budget: ReplyBudget,
    quote: CostReservation,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO reply_money_daily(user_id,day,currency,occupied) VALUES($1,$2::text::date,$3,0) ON CONFLICT DO NOTHING")
        .bind(owner).bind(day).bind(&budget.currency).execute(&mut *tx).await.map_err(map_error)?;
    let occupied: i64 = sqlx::query_scalar("SELECT occupied FROM reply_money_daily WHERE user_id=$1 AND day=$2::text::date AND currency=$3 FOR UPDATE")
        .bind(owner).bind(day).bind(&budget.currency).fetch_one(&mut *tx).await.map_err(map_error)?;
    let total = quote
        .reserve_against(occupied, budget.daily_limit)
        .map_err(budget_error)?;
    sqlx::query("UPDATE reply_money_daily SET occupied=$4 WHERE user_id=$1 AND day=$2::text::date AND currency=$3")
        .bind(owner).bind(day).bind(&budget.currency).bind(total).execute(&mut *tx).await.map_err(map_error)?;
    let json = serde_json::to_string(&budget)
        .map_err(|_| StorageError::InvalidData("invalid reply money configuration".into()))?;
    sqlx::query("INSERT INTO reply_money_reservations(user_id,conversation_id,request_id,day,currency,model,configuration_revision,budget,reserved) VALUES($1,$2,$3,$4::text::date,$5,$6,$7,$8::text::jsonb,$9)")
        .bind(owner).bind(ids.0).bind(ids.1).bind(day).bind(&budget.currency).bind(&configuration.model).bind(&configuration.revision).bind(json).bind(quote.amount())
        .execute(tx).await.map_err(map_error)?;
    Ok(())
}

pub(super) async fn settle(
    tx: &mut PgConnection,
    owner: Uuid,
    conversation: Uuid,
    request: Uuid,
    before_dispatch: bool,
    usage: Option<ReplyUsage>,
) -> StorageResult<()> {
    let Some(row) = sqlx::query("SELECT day::text AS day,currency,budget::text AS budget,reserved FROM reply_money_reservations WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3 AND charged IS NULL FOR UPDATE")
        .bind(owner).bind(conversation).bind(request).fetch_optional(&mut *tx).await.map_err(map_error)? else { return Ok(()); };
    let budget: ReplyBudget = serde_json::from_str(row.get("budget"))
        .map_err(|_| StorageError::Unavailable("invalid stored reply budget".into()))?;
    let quote = quote(&budget)?;
    let reserved: i64 = row.get("reserved");
    if quote.amount() != reserved {
        return Err(StorageError::Unavailable(
            "inconsistent reply budget".into(),
        ));
    }
    let (charged, reason) = if before_dispatch {
        (0, "cancelled_before_dispatch")
    } else {
        match quote.settle(usage.map(|u| BillableUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        })) {
            Settlement::Verified { charged, .. } => (charged, "verified"),
            Settlement::RetainReservation { charged } => (
                charged,
                if usage.is_some() {
                    "usage_exceeded"
                } else {
                    "retained"
                },
            ),
        }
    };
    let updated = sqlx::query("UPDATE reply_money_daily SET occupied=occupied-$4 WHERE user_id=$1 AND day=$2::text::date AND currency=$3 AND occupied >= $4")
        .bind(owner).bind(row.get::<&str,_>("day")).bind(row.get::<&str,_>("currency")).bind(reserved-charged).execute(&mut *tx).await.map_err(map_error)?;
    if updated.rows_affected() != 1 {
        return Err(StorageError::Unavailable(
            "missing reply money ledger".into(),
        ));
    }
    sqlx::query("UPDATE reply_money_reservations SET charged=$4,settlement=$5,settled_at=clock_timestamp() WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3 AND charged IS NULL")
        .bind(owner).bind(conversation).bind(request).bind(charged).bind(reason).execute(tx).await.map_err(map_error)?;
    Ok(())
}

pub(super) async fn delete_conversation(
    tx: &mut PgConnection,
    owner: Uuid,
    conversation: Uuid,
) -> StorageResult<()> {
    let rows = sqlx::query("SELECT r.request_id,r.status FROM conversation_replies r JOIN reply_money_reservations b USING(conversation_id,request_id) WHERE b.user_id=$1 AND r.conversation_id=$2 AND b.charged IS NULL")
        .bind(owner).bind(conversation).fetch_all(&mut *tx).await.map_err(map_error)?;
    for row in rows {
        settle(
            tx,
            owner,
            conversation,
            row.get("request_id"),
            row.get::<&str, _>("status") == "queued",
            None,
        )
        .await?;
    }
    Ok(())
}
