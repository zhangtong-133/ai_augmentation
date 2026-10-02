//! 有期限的 RSS 周期采集授权及到期时段计算；不领取任务或发送网络请求。
use crate::{
    APPROVAL_TTL_MS, CollectionPolicy, PlanError, SubscriptionSnapshot, identity, normalize_source,
};
use personal_ai_domain::UserId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEDULE_VERSION: &str = "rss-schedule-v1";
pub const MAX_AUTHORIZATION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const DISPATCH_WINDOW_MS: u64 = 10 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInput {
    pub schedule_id: String,
    pub starts_at_unix_ms: u64,
    pub ends_at_unix_ms: u64,
    pub interval_hours: u8,
}

/// 可保存的预览。摘要绑定来源、订阅版本、频率、期限及固定采集限制。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulePlan {
    pub version: String,
    pub user_id: String,
    pub subscription_id: String,
    pub subscription_revision: u64,
    pub source_url: String,
    pub input: ScheduleInput,
    pub created_at_unix_ms: u64,
    pub approval_expires_at_unix_ms: u64,
    pub max_occurrences: u64,
    pub dispatch_window_ms: u64,
    pub policy: CollectionPolicy,
}
impl SchedulePlan {
    /// # Errors
    /// 无法编码预览时返回 `InvalidPlan`。摘要不是认证签名。
    pub fn consent_digest(&self) -> Result<String, PlanError> {
        let bytes = serde_json::to_vec(self).map_err(|_| PlanError::InvalidPlan)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

/// 只由精确同意校验产生；不能从客户端 JSON 构造执行授权。
#[derive(Clone)]
pub struct AuthorizedSchedule {
    plan: SchedulePlan,
}

/// 一次候选时段，不是网络执行凭据；仓储仍需原子去重、额度预留和领取。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledOccurrence {
    pub occurrence_key: String,
    pub scheduled_at_unix_ms: u64,
    pub dispatch_expires_at_unix_ms: u64,
}

/// 生成周期采集预览，不默认授权或执行。开始时间至少晚于预览五分钟，
/// 结束时间不超过预览后七天，频率固定为每 1、6 或 24 小时。
/// # Errors
/// 拒绝跨用户、停用/无效订阅、非法频率、时间范围或整数溢出。
pub fn plan_schedule(
    user: &UserId,
    subscription: &SubscriptionSnapshot,
    input: &ScheduleInput,
    now_ms: u64,
) -> Result<SchedulePlan, PlanError> {
    let owner = identity(user.as_str())?;
    if owner != identity(&subscription.user_id)? {
        return Err(PlanError::OwnerMismatch);
    }
    if !subscription.enabled {
        return Err(PlanError::Disabled);
    }
    if subscription.revision == 0 {
        return Err(PlanError::InvalidRevision);
    }
    if ![1, 6, 24].contains(&input.interval_hours) {
        return Err(PlanError::InvalidPlan);
    }
    let approval_end = now_ms
        .checked_add(APPROVAL_TTL_MS)
        .ok_or(PlanError::InvalidTime)?;
    let authorization_end = now_ms
        .checked_add(MAX_AUTHORIZATION_MS)
        .ok_or(PlanError::InvalidTime)?;
    if input.starts_at_unix_ms < approval_end
        || input.ends_at_unix_ms <= input.starts_at_unix_ms
        || input.ends_at_unix_ms > authorization_end
    {
        return Err(PlanError::InvalidTime);
    }
    let mut input = input.clone();
    input.schedule_id = identity(&input.schedule_id)?;
    let count = (input.ends_at_unix_ms - input.starts_at_unix_ms)
        .div_ceil(u64::from(input.interval_hours) * HOUR_MS);
    Ok(SchedulePlan {
        version: SCHEDULE_VERSION.into(),
        user_id: owner,
        subscription_id: identity(&subscription.subscription_id)?,
        subscription_revision: subscription.revision,
        source_url: normalize_source(&subscription.source_url)?,
        input,
        created_at_unix_ms: now_ms,
        approval_expires_at_unix_ms: approval_end,
        max_occurrences: count,
        dispatch_window_ms: DISPATCH_WINDOW_MS,
        policy: CollectionPolicy::default(),
    })
}

/// 对持久化预览进行精确同意校验；调用方还须验证会话并事务保存同意。
/// # Errors
/// 拒绝篡改、跨用户、变化/停用的订阅、未来/过期预览或错误摘要。
pub fn authorize_schedule(
    user: &UserId,
    current: &SubscriptionSnapshot,
    saved: &SchedulePlan,
    accepted_digest: &str,
    now_ms: u64,
) -> Result<AuthorizedSchedule, PlanError> {
    let expected = plan_schedule(user, current, &saved.input, saved.created_at_unix_ms)?;
    if expected.user_id != saved.user_id {
        return Err(PlanError::OwnerMismatch);
    }
    if expected.subscription_id != saved.subscription_id
        || expected.subscription_revision != saved.subscription_revision
        || expected.source_url != saved.source_url
    {
        return Err(PlanError::StaleSubscription);
    }
    if expected != *saved {
        return Err(PlanError::InvalidPlan);
    }
    if now_ms < saved.created_at_unix_ms {
        return Err(PlanError::InvalidTime);
    }
    if now_ms >= saved.approval_expires_at_unix_ms {
        return Err(PlanError::Expired);
    }
    if accepted_digest != saved.consent_digest()? {
        return Err(PlanError::ConsentMismatch);
    }
    Ok(AuthorizedSchedule {
        plan: saved.clone(),
    })
}

impl AuthorizedSchedule {
    /// 仅给出当前时段候选；重复调用返回同一 key，不能代替数据库一次性领取。
    /// `active` 必须取自当前持久化启停状态；过期、停用、未到期或错过窗口返回空。
    /// # Errors
    /// 订阅归属、版本或来源变化时拒绝继续使用旧授权。
    pub fn due(
        &self,
        user: &UserId,
        current: &SubscriptionSnapshot,
        active: bool,
        now_ms: u64,
    ) -> Result<Option<ScheduledOccurrence>, PlanError> {
        if !active || !current.enabled {
            return Ok(None);
        }
        let plan = &self.plan;
        let expected = plan_schedule(user, current, &plan.input, plan.created_at_unix_ms)?;
        if expected.user_id != plan.user_id {
            return Err(PlanError::OwnerMismatch);
        }
        if expected != *plan {
            return Err(PlanError::StaleSubscription);
        }
        if now_ms < plan.input.starts_at_unix_ms || now_ms >= plan.input.ends_at_unix_ms {
            return Ok(None);
        }
        let interval = u64::from(plan.input.interval_hours) * HOUR_MS;
        let slot = (now_ms - plan.input.starts_at_unix_ms) / interval;
        let scheduled = plan.input.starts_at_unix_ms + slot * interval;
        let deadline = scheduled
            .saturating_add(DISPATCH_WINDOW_MS)
            .min(plan.input.ends_at_unix_ms);
        if now_ms >= deadline {
            return Ok(None);
        }
        let key = format!("{}:{scheduled}", plan.consent_digest()?);
        Ok(Some(ScheduledOccurrence {
            occurrence_key: format!("{:x}", Sha256::digest(key.as_bytes())),
            scheduled_at_unix_ms: scheduled,
            dispatch_expires_at_unix_ms: deadline,
        }))
    }
}

#[cfg(test)]
mod tests;
