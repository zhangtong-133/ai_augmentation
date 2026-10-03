use super::{PendingLogin, Registration, TokenResponse, credentials};
use reqwest::Url;
use uuid::Uuid;

#[test]
fn callback_binds_state_client_and_pkce() {
    let host = Uuid::new_v4().urn().to_string();
    let (pending, url) = PendingLogin::new(&host, 1234, None).unwrap();
    let url = Url::parse(&url).unwrap();
    let p: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(p["client_id"], "dynamic_agent_client");
    assert_eq!(p["redirect_uri"], "http://127.0.0.1:1234/auth/callback");
    assert_eq!(p["code_challenge_method"], "S256");
    assert_eq!(p["code_challenge"].len(), 43);
    let query = format!(
        "state={}&code=secret-code&client_id=oaiapp_test",
        p["state"]
    );
    assert!(!pending.matches_state("state=wrong"));
    assert!(!pending.matches_state(&format!("{query}&state=duplicate")));
    assert!(
        pending
            .clone()
            .callback(&query.replace("oaiapp_test", "dynamic_agent_client"))
            .is_err()
    );
    assert!(
        pending
            .clone()
            .callback(&format!("{query}&error=access_denied"))
            .is_err()
    );
    assert_eq!(
        pending.callback(&query).unwrap().registration.client_id,
        "oaiapp_test"
    );
    let registration = Registration::pending("oaiapp_existing".into());
    let (pending, url) = PendingLogin::new(&host, 1234, Some(&registration)).unwrap();
    assert!(!url.contains("agent_name_hint"));
    let query = format!("state={}&code=new-code", pending.state);
    assert!(
        pending
            .clone()
            .callback(&format!("{query}&client_id=oaiapp_other"))
            .is_err()
    );
    assert_eq!(
        pending.callback(&query).unwrap().registration.client_id,
        "oaiapp_existing"
    );
}
#[test]
fn granted_scopes_and_rotated_refresh_are_persisted_together() {
    let mut r = Registration::pending("oaiapp_test".into());
    let old: TokenResponse = serde_json::from_value(serde_json::json!({"access_token":"old","refresh_token":"refresh-old","token_type":"Bearer","expires_in":3600,"scope":"openid resource.invoke chatgpt.tokens.use.direct"})).unwrap();
    r.credentials = Some(credentials(old, None, super::now().unwrap()).unwrap());
    assert!(r.plan_enabled());
    let token: TokenResponse = serde_json::from_value(serde_json::json!({"access_token":"new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600})).unwrap();
    r.credentials =
        Some(credentials(token, r.credentials.as_ref(), super::now().unwrap()).unwrap());
    assert!(r.plan_enabled());
    assert_eq!(
        r.credentials.as_ref().unwrap().refresh_token.as_deref(),
        Some("refresh-new")
    );
    let token: TokenResponse = serde_json::from_value(serde_json::json!({"access_token":"new","token_type":"Bearer","expires_in":3600,"scope":"openid"})).unwrap();
    r.credentials =
        Some(credentials(token, r.credentials.as_ref(), super::now().unwrap()).unwrap());
    assert!(!r.plan_enabled());
    assert!(r.access_token().is_err());
}
#[test]
fn missing_or_expired_permissions_cannot_send() {
    let mut r = Registration::pending("oaiapp_test".into());
    assert!(r.access_token().is_err());
    let token: TokenResponse = serde_json::from_value(serde_json::json!({"access_token":"expired","token_type":"Bearer","expires_in":1,"scope":"resource.invoke chatgpt.tokens.use.direct"})).unwrap();
    r.credentials = Some(credentials(token, None, 1).unwrap());
    assert!(r.access_token().is_err());
}

#[test]
fn verifies_signature_issuer_audience_expiry_nonce_and_authorized_party() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, jwk::JwkSet};
    let encoded: String = include_str!("fixtures/test-only-private.pem")
        .lines()
        .filter(|s| !s.starts_with("---"))
        .collect();
    let key = EncodingKey::from_rsa_der(&STANDARD.decode(encoded).unwrap());
    let keys: JwkSet = serde_json::from_str(include_str!("fixtures/jwks.json")).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-only".into());
    let claims = serde_json::json!({"iss":super::AUTH,"aud":"oaiapp_test","sub":"test-user","exp":super::now().unwrap()+3600,"nonce":"nonce","azp":"oaiapp_test"});
    let sign = |value: &serde_json::Value| encode(&header, value, &key).unwrap();
    assert!(super::verify_claims(&sign(&claims), "oaiapp_test", Some("nonce"), &keys).is_ok());
    for (field, value) in [
        ("iss", serde_json::json!("https://untrusted.example")),
        ("aud", serde_json::json!("oaiapp_other")),
        ("exp", serde_json::json!(1)),
        ("nbf", serde_json::json!(super::now().unwrap() + 3600)),
        ("nonce", serde_json::json!("wrong")),
        ("azp", serde_json::json!("oaiapp_other")),
        ("sub", serde_json::json!("")),
    ] {
        let mut changed = claims.clone();
        changed[field] = value;
        assert!(
            super::verify_claims(&sign(&changed), "oaiapp_test", Some("nonce"), &keys).is_err(),
            "{field}"
        );
    }
    let mut multiple = claims.clone();
    multiple["aud"] = serde_json::json!(["oaiapp_test", "oaiapp_other"]);
    multiple.as_object_mut().unwrap().remove("azp");
    assert!(super::verify_claims(&sign(&multiple), "oaiapp_test", Some("nonce"), &keys).is_err());
    let mut token = sign(&claims).into_bytes();
    let last = token.len() - 10;
    token[last] = if token[last] == b'A' { b'B' } else { b'A' };
    assert!(
        super::verify_claims(
            std::str::from_utf8(&token).unwrap(),
            "oaiapp_test",
            Some("nonce"),
            &keys
        )
        .is_err()
    );
    let unsigned = format!("e30.e30.{}", "A".repeat(50));
    assert!(super::verify_claims(&unsigned, "oaiapp_test", Some("nonce"), &keys).is_err());
}
