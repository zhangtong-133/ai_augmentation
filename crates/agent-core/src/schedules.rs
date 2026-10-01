//! 提醒授权指纹绑定归属、幂等 ID、时间、内容和固定的零费用/单次策略。
use personal_ai_domain::UserId;
use personal_ai_storage::schedules::{NewSchedule, SCHEDULE_VERSION};
use sha2::{Digest, Sha256};

#[must_use]
pub fn schedule_digest(owner: &UserId, input: &NewSchedule) -> String {
    let definition = serde_json::json!({"owner":owner.as_str(),"input":input,
        "version":SCHEDULE_VERSION,"max_runs":1,"amount_micro":0});
    format!("{:x}", Sha256::digest(definition.to_string().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consent_binds_identity_content_time_and_request() {
        let owner = UserId::new("owner");
        let input = NewSchedule {
            request_id: "id".into(),
            title: "提醒".into(),
            body: "复习".into(),
            run_at_unix_ms: 123,
        };
        let digest = schedule_digest(&owner, &input);
        assert_eq!(digest, schedule_digest(&owner, &input));
        assert_ne!(digest, schedule_digest(&UserId::new("other"), &input));
        let mut changed = input.clone();
        changed.run_at_unix_ms += 1;
        assert_ne!(digest, schedule_digest(&owner, &changed));
        changed = input.clone();
        changed.body.push('!');
        assert_ne!(digest, schedule_digest(&owner, &changed));
        changed = input.clone();
        changed.title.push('!');
        assert_ne!(digest, schedule_digest(&owner, &changed));
        changed = input;
        changed.request_id.push('!');
        assert_ne!(digest, schedule_digest(&owner, &changed));
    }
}
