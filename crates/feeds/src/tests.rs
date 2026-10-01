use super::*;

fn fixture() -> (UserId, SubscriptionSnapshot, String) {
    let user = UserId::new(Uuid::new_v4().to_string());
    let source = SubscriptionSnapshot {
        user_id: user.to_string(),
        subscription_id: Uuid::new_v4().to_string(),
        revision: 1,
        source_url: "https://EXAMPLE.com:443/feed.xml#section".into(),
        enabled: true,
    };
    (user, source, Uuid::new_v4().to_string())
}
#[test]
fn sources_are_canonical_but_never_claim_to_validate_dns() {
    assert_eq!(
        normalize_source("https://EXAMPLE.com:443/feed.xml?a=1&b=2#x").unwrap(),
        "https://example.com/feed.xml?a=1&b=2"
    );
    for url in [
        "http://example.com",
        "file:///etc/passwd",
        "https://example.com:8443/feed",
        "https://user:password@example.com",
        "https://127.0.0.1",
        "https://2130706433",
        "https://[::1]",
        "https://localhost",
        "https://host.local",
        "https://host.internal",
        "https://host.test",
        "https://a..com",
        "https://example.com.",
        " https://example.com",
        "https://example.com/\nsecret",
    ] {
        assert_eq!(normalize_source(url), Err(PlanError::InvalidSource));
    }
    assert_eq!(
        normalize_source(&format!("https://example.com/{}", "x".repeat(2048))),
        Err(PlanError::InvalidSource)
    );
}
#[test]
fn explicit_consent_binds_source_owner_request_revision_and_fixed_limits() {
    let (user, subscription, request) = fixture();
    let plan = plan_collection(&user, &subscription, &request, 1000).unwrap();
    let digest = plan.consent_digest().unwrap();
    assert_eq!(plan.source_url, "https://example.com/feed.xml");
    assert_eq!(plan.policy.max_redirects, 0);
    assert!(!plan.policy.call_models);
    assert_eq!(
        validate_approval(&user, &subscription, &plan, &digest, 1000),
        Ok(())
    );
    assert_eq!(
        validate_approval(&user, &subscription, &plan, "wrong", 1000),
        Err(PlanError::ConsentMismatch)
    );
    let second = plan_collection(&user, &subscription, &Uuid::new_v4().to_string(), 1000).unwrap();
    assert_ne!(second.consent_digest().unwrap(), digest);
    let foreign = UserId::new(Uuid::new_v4().to_string());
    assert!(matches!(
        plan_collection(&foreign, &subscription, &request, 1000),
        Err(PlanError::OwnerMismatch)
    ));
    let mut changed = subscription.clone();
    changed.revision += 1;
    assert_eq!(
        validate_approval(&user, &changed, &plan, &digest, 1000),
        Err(PlanError::StaleSubscription)
    );
    changed = subscription.clone();
    changed.source_url = "https://example.com/new.xml".into();
    assert_eq!(
        validate_approval(&user, &changed, &plan, &digest, 1000),
        Err(PlanError::StaleSubscription)
    );
    changed = subscription;
    changed.enabled = false;
    assert_eq!(
        validate_approval(&user, &changed, &plan, &digest, 1000),
        Err(PlanError::Disabled)
    );
}
#[test]
fn tampered_policy_or_lifetime_is_rejected_even_with_recomputed_digest() {
    let (user, subscription, request) = fixture();
    let plan = plan_collection(&user, &subscription, &request, 1000).unwrap();
    let mut variants = vec![plan.clone(); 5];
    variants[0].policy.call_models = true;
    variants[1].policy.max_response_bytes += 1;
    variants[2].policy.max_redirects = 1;
    variants[3].approval_expires_at_unix_ms += 1;
    variants[4].version = "other".into();
    for altered in variants {
        assert_eq!(
            validate_approval(
                &user,
                &subscription,
                &altered,
                &altered.consent_digest().unwrap(),
                1000
            ),
            Err(PlanError::InvalidPlan)
        );
    }
    let digest = plan.consent_digest().unwrap();
    assert_eq!(
        validate_approval(&user, &subscription, &plan, &digest, 999),
        Err(PlanError::InvalidTime)
    );
    assert_eq!(
        validate_approval(&user, &subscription, &plan, &digest, 300_999),
        Ok(())
    );
    assert_eq!(
        validate_approval(&user, &subscription, &plan, &digest, 301_000),
        Err(PlanError::Expired)
    );
    assert!(matches!(
        plan_collection(&user, &subscription, &request, u64::MAX),
        Err(PlanError::InvalidTime)
    ));
}
#[test]
fn serialized_plan_roundtrips_and_unknown_fields_do_not_expand_permissions() {
    let (user, subscription, request) = fixture();
    let plan = plan_collection(&user, &subscription, &request, 1000).unwrap();
    let json = serde_json::to_string(&plan).unwrap();
    let saved: CollectionPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(
        saved.consent_digest().unwrap(),
        plan.consent_digest().unwrap()
    );
    let mut value = serde_json::to_value(plan).unwrap();
    value["policy"]["headers"] = serde_json::json!({"Authorization":"private"});
    assert!(serde_json::from_value::<CollectionPlan>(value).is_err());
    // Pure validation intentionally does not implement a replay ledger.
    let digest = saved.consent_digest().unwrap();
    for _ in 0..2 {
        assert_eq!(
            validate_approval(&user, &subscription, &saved, &digest, 1000),
            Ok(())
        );
    }
}

#[test]
fn invalid_snapshot_identity_revision_and_subscription_substitution_fail_closed() {
    let (user, mut source, request) = fixture();
    let saved = plan_collection(&user, &source, &request, 1000).unwrap();
    let digest = saved.consent_digest().unwrap();
    source.subscription_id = Uuid::new_v4().to_string();
    assert_eq!(
        validate_approval(&user, &source, &saved, &digest, 1000),
        Err(PlanError::StaleSubscription)
    );
    source.revision = 0;
    assert!(matches!(
        plan_collection(&user, &source, &request, 1000),
        Err(PlanError::InvalidRevision)
    ));
    source.revision = 1;
    source.subscription_id = Uuid::nil().to_string();
    assert!(matches!(
        plan_collection(&user, &source, &request, 1000),
        Err(PlanError::InvalidIdentity)
    ));
    source.subscription_id = Uuid::new_v4().to_string();
    assert!(matches!(
        plan_collection(&user, &source, "not-a-uuid", 1000),
        Err(PlanError::InvalidIdentity)
    ));
}
