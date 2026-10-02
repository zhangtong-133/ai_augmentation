use super::*;

fn fixture() -> (UserId, SubscriptionSnapshot, ScheduleInput) {
    let owner = "10000000-0000-4000-8000-000000000001";
    (
        UserId::new(owner),
        SubscriptionSnapshot {
            user_id: owner.into(),
            subscription_id: "20000000-0000-4000-8000-000000000001".into(),
            revision: 1,
            source_url: "https://example.org/feed.xml".into(),
            enabled: true,
        },
        ScheduleInput {
            schedule_id: "30000000-0000-4000-8000-000000000001".into(),
            starts_at_unix_ms: APPROVAL_TTL_MS,
            ends_at_unix_ms: MAX_AUTHORIZATION_MS,
            interval_hours: 1,
        },
    )
}
fn approved(
    user: &UserId,
    source: &SubscriptionSnapshot,
    input: &ScheduleInput,
) -> AuthorizedSchedule {
    let plan = plan_schedule(user, source, input, 0).unwrap();
    authorize_schedule(user, source, &plan, &plan.consent_digest().unwrap(), 1).unwrap()
}

#[test]
fn explicit_consent_binds_bounded_frequency_source_and_policy() {
    let (user, source, mut input) = fixture();
    for (interval, count) in [(1, 168), (6, 28), (24, 7)] {
        input.interval_hours = interval;
        let plan = plan_schedule(&user, &source, &input, 0).unwrap();
        assert_eq!(plan.max_occurrences, count);
        assert_eq!(plan.policy, CollectionPolicy::default());
        assert_eq!(plan.dispatch_window_ms, DISPATCH_WINDOW_MS);
        assert!(matches!(
            authorize_schedule(&user, &source, &plan, "", 1),
            Err(PlanError::ConsentMismatch)
        ));
        let digest = plan.consent_digest().unwrap();
        assert!(authorize_schedule(&user, &source, &plan, &digest, 1).is_ok());
        let mut changed = plan.clone();
        changed.policy.call_models = true;
        assert!(matches!(
            authorize_schedule(
                &user,
                &source,
                &changed,
                &changed.consent_digest().unwrap(),
                1
            ),
            Err(PlanError::InvalidPlan)
        ));
        let mut changed = plan.clone();
        changed.max_occurrences += 1;
        assert!(matches!(
            authorize_schedule(&user, &source, &changed, &digest, 1),
            Err(PlanError::InvalidPlan)
        ));
        let mut changed = plan.clone();
        changed.input.ends_at_unix_ms -= 1;
        // 即使字段修改后仍构成有效预览，也必须重新同意新的摘要。
        let revised = plan_schedule(&user, &source, &changed.input, 0).unwrap();
        assert!(matches!(
            authorize_schedule(&user, &source, &revised, &digest, 1),
            Err(PlanError::ConsentMismatch)
        ));
    }
}

#[test]
fn due_windows_are_half_open_stable_and_never_catch_up_missed_slots() {
    let (user, source, input) = fixture();
    let authorized = approved(&user, &source, &input);
    let start = input.starts_at_unix_ms;
    assert!(
        authorized
            .due(&user, &source, true, start - 1)
            .unwrap()
            .is_none()
    );
    let first = authorized
        .due(&user, &source, true, start)
        .unwrap()
        .unwrap();
    assert_eq!(
        first,
        authorized
            .due(&user, &source, true, start + 1)
            .unwrap()
            .unwrap()
    );
    assert_eq!(
        first.dispatch_expires_at_unix_ms,
        start + DISPATCH_WINDOW_MS
    );
    assert!(
        authorized
            .due(&user, &source, true, start + DISPATCH_WINDOW_MS)
            .unwrap()
            .is_none()
    );
    let later = authorized
        .due(&user, &source, true, start + 3 * HOUR_MS)
        .unwrap()
        .unwrap();
    assert_eq!(later.scheduled_at_unix_ms, start + 3 * HOUR_MS);
    assert_ne!(later.occurrence_key, first.occurrence_key);
    assert!(
        authorized
            .due(&user, &source, true, input.ends_at_unix_ms)
            .unwrap()
            .is_none()
    );
    assert!(
        authorized
            .due(&user, &source, true, u64::MAX)
            .unwrap()
            .is_none()
    );
    let mut short = input;
    short.ends_at_unix_ms = start + 1;
    let short_plan = approved(&user, &source, &short);
    assert_eq!(
        short_plan
            .due(&user, &source, true, start)
            .unwrap()
            .unwrap()
            .dispatch_expires_at_unix_ms,
        start + 1
    );
    assert!(
        short_plan
            .due(&user, &source, true, start + 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn expiry_revocation_and_changed_subscriptions_block_old_authorization() {
    let (user, source, input) = fixture();
    let plan = plan_schedule(&user, &source, &input, 0).unwrap();
    let digest = plan.consent_digest().unwrap();
    assert!(matches!(
        authorize_schedule(&user, &source, &plan, &digest, APPROVAL_TTL_MS),
        Err(PlanError::Expired)
    ));
    let future = plan_schedule(
        &user,
        &source,
        &ScheduleInput {
            starts_at_unix_ms: APPROVAL_TTL_MS + 10,
            ..input.clone()
        },
        10,
    )
    .unwrap();
    assert!(matches!(
        authorize_schedule(
            &user,
            &source,
            &future,
            &future.consent_digest().unwrap(),
            9
        ),
        Err(PlanError::InvalidTime)
    ));
    let authorized = approved(&user, &source, &input);
    assert!(
        authorized
            .due(&user, &source, false, input.starts_at_unix_ms)
            .unwrap()
            .is_none()
    );
    let mut changed = source.clone();
    changed.enabled = false;
    assert!(
        authorized
            .due(&user, &changed, true, input.starts_at_unix_ms)
            .unwrap()
            .is_none()
    );
    for change in [0, 1, 2] {
        let mut changed = source.clone();
        match change {
            0 => changed.revision += 1,
            1 => changed.source_url = "https://example.com/other.xml".into(),
            _ => changed.subscription_id = "20000000-0000-4000-8000-000000000002".into(),
        }
        assert!(matches!(
            authorized.due(&user, &changed, true, input.starts_at_unix_ms),
            Err(PlanError::StaleSubscription)
        ));
        assert!(matches!(
            authorize_schedule(&user, &changed, &plan, &digest, 1),
            Err(PlanError::StaleSubscription)
        ));
    }
}

#[test]
fn owners_and_new_authorizations_have_separate_occurrence_keys() {
    let (user, source, input) = fixture();
    let original = approved(&user, &source, &input);
    let other = UserId::new("10000000-0000-4000-8000-000000000002");
    assert!(matches!(
        original.due(&other, &source, true, input.starts_at_unix_ms),
        Err(PlanError::OwnerMismatch)
    ));
    let mut other_source = source.clone();
    other_source.user_id = other.as_str().into();
    assert!(matches!(
        original.due(&other, &other_source, true, input.starts_at_unix_ms),
        Err(PlanError::OwnerMismatch)
    ));
    let other_auth = approved(&other, &other_source, &input);
    let key = original
        .due(&user, &source, true, input.starts_at_unix_ms)
        .unwrap()
        .unwrap()
        .occurrence_key;
    assert_ne!(
        key,
        other_auth
            .due(&other, &other_source, true, input.starts_at_unix_ms)
            .unwrap()
            .unwrap()
            .occurrence_key
    );
    let mut new_input = input.clone();
    new_input.schedule_id = "30000000-0000-4000-8000-000000000002".into();
    assert_ne!(
        key,
        approved(&user, &source, &new_input)
            .due(&user, &source, true, input.starts_at_unix_ms)
            .unwrap()
            .unwrap()
            .occurrence_key
    );
}

#[test]
fn invalid_ranges_frequency_identity_and_sources_never_produce_plans() {
    let (user, source, input) = fixture();
    for hours in [0, 2, 7, 255] {
        let mut bad = input.clone();
        bad.interval_hours = hours;
        assert!(matches!(
            plan_schedule(&user, &source, &bad, 0),
            Err(PlanError::InvalidPlan)
        ));
    }
    for (start, end) in [
        (0, MAX_AUTHORIZATION_MS),
        (APPROVAL_TTL_MS, APPROVAL_TTL_MS),
        (MAX_AUTHORIZATION_MS, APPROVAL_TTL_MS),
        (APPROVAL_TTL_MS, MAX_AUTHORIZATION_MS + 1),
    ] {
        let mut bad = input.clone();
        bad.starts_at_unix_ms = start;
        bad.ends_at_unix_ms = end;
        assert!(matches!(
            plan_schedule(&user, &source, &bad, 0),
            Err(PlanError::InvalidTime)
        ));
    }
    assert!(matches!(
        plan_schedule(&user, &source, &input, u64::MAX),
        Err(PlanError::InvalidTime)
    ));
    let mut bad = input;
    bad.schedule_id = "00000000-0000-0000-0000-000000000000".into();
    assert!(matches!(
        plan_schedule(&user, &source, &bad, 0),
        Err(PlanError::InvalidIdentity)
    ));
    let (_, _, input) = fixture();
    let mut bad = source.clone();
    bad.source_url = "http://127.0.0.1/feed".into();
    assert!(matches!(
        plan_schedule(&user, &bad, &input, 0),
        Err(PlanError::InvalidSource)
    ));
    let mut bad = source;
    bad.revision = 0;
    assert!(matches!(
        plan_schedule(&user, &bad, &input, 0),
        Err(PlanError::InvalidRevision)
    ));
}
