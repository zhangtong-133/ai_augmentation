//! The borrowed private store keeps the same account locked throughout verification and inference.
use super::store::Store;
use personal_ai_agent_core::feed_value_execution::{SubscriptionValueRuntime, ValueRuntimeError};
use personal_ai_llm::ChatRequest;
use personal_ai_llm_openai::chatgpt::{ChatGptClient, Registration, Result};
use personal_ai_storage::{
    BoxFuture, feed_value::ValuePricing, subscription_connections::VerifiedSubscriptionConnection,
};

pub(super) trait ValueClient: Send + Sync {
    fn models<'a>(&'a self, registration: &'a Registration) -> BoxFuture<'a, Result<Vec<String>>>;
    fn score<'a>(
        &'a self,
        registration: &'a Registration,
        model: &'a str,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<String>>;
}
impl ValueClient for ChatGptClient {
    fn models<'a>(&'a self, registration: &'a Registration) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(async move {
            self.models(registration)
                .await
                .map(|models| models.into_iter().map(|m| m.slug).collect())
        })
    }
    fn score<'a>(
        &'a self,
        registration: &'a Registration,
        model: &'a str,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { self.score_value(registration, model, request, true).await })
    }
}
pub(super) struct Runtime<'a> {
    pub client: &'a dyn ValueClient,
    pub store: &'a Store,
    pub label: &'a str,
}
fn model(pricing: &ValuePricing) -> std::result::Result<&str, ValueRuntimeError> {
    match pricing {
        ValuePricing::Subscription {
            provider, model, ..
        } if provider == "chatgpt-plan" => Ok(model),
        _ => Err(ValueRuntimeError),
    }
}
impl SubscriptionValueRuntime for Runtime<'_> {
    fn verify<'a>(
        &'a self,
        pricing: &'a ValuePricing,
    ) -> BoxFuture<'a, std::result::Result<VerifiedSubscriptionConnection, ValueRuntimeError>> {
        Box::pin(async move {
            let model = model(pricing)?;
            let registration = self
                .store
                .data
                .accounts
                .get(self.label)
                .ok_or(ValueRuntimeError)?;
            let expiry = registration
                .access_expires_at()
                .map_err(|_| ValueRuntimeError)?;
            let models = self
                .client
                .models(registration)
                .await
                .map_err(|_| ValueRuntimeError)?;
            if !models.iter().any(|m| m == model) {
                return Err(ValueRuntimeError);
            }
            Ok(VerifiedSubscriptionConnection {
                host_id: self.store.data.host_id.clone(),
                client_id: registration.client_id.clone(),
                subject: registration.subject.clone().ok_or(ValueRuntimeError)?,
                label: self.label.into(),
                models,
                valid_until_unix_ms: i64::try_from(expiry)
                    .ok()
                    .and_then(|s| s.checked_mul(1000))
                    .ok_or(ValueRuntimeError)?,
            })
        })
    }
    fn score<'a>(
        &'a self,
        pricing: &'a ValuePricing,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, std::result::Result<Vec<u8>, ValueRuntimeError>> {
        Box::pin(async move {
            let registration = self
                .store
                .data
                .accounts
                .get(self.label)
                .ok_or(ValueRuntimeError)?;
            self.client
                .score(registration, model(pricing)?, request)
                .await
                .map(String::into_bytes)
                .map_err(|_| ValueRuntimeError)
        })
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    pub struct Client {
        pub calls: AtomicUsize,
    }
    impl ValueClient for Client {
        fn models<'a>(&'a self, _: &'a Registration) -> BoxFuture<'a, Result<Vec<String>>> {
            Box::pin(async { Ok(vec!["fixture".into()]) })
        }
        fn score<'a>(
            &'a self,
            registration: &'a Registration,
            model: &'a str,
            request: &'a ChatRequest,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                assert_eq!(registration.subject.as_deref(), Some("fixture-user"));
                assert_eq!(model, "fixture");
                assert_eq!(request.messages.len(), 2);
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(r#"{"items":[{"id":1,"score":null,"reason":"insufficient evidence"}]}"#.into())
            })
        }
    }
    pub fn local() -> (std::path::PathBuf, Store) {
        let path = std::env::temp_dir().join(format!("value-runtime-{}", uuid::Uuid::new_v4()));
        let mut store = Store::open(&path).unwrap();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let registration=serde_json::from_value(serde_json::json!({"client_id":format!("oaiapp_{}",uuid::Uuid::new_v4().simple()),"subject":"fixture-user","email":null,"credentials":{"access_token":"test-only","refresh_token":"test-refresh","expires_at":time+1800,"scopes":["resource.invoke","chatgpt.tokens.use.direct"]}})).unwrap();
        store.data.accounts.insert("fixture".into(), registration);
        store.save().unwrap();
        (path, store)
    }
    pub fn pricing(connection: String) -> ValuePricing {
        ValuePricing::Subscription {
            provider: "chatgpt-plan".into(),
            model: "fixture".into(),
            configuration_version: "connection-v1".into(),
            connection_id: connection,
            valid_until_unix_ms: i64::MAX,
        }
    }
    #[tokio::test]
    async fn runtime_keeps_account_locked_and_rejects_missing_scope_or_model() {
        let (path, mut store) = local();
        let client = Client {
            calls: AtomicUsize::new(0),
        };
        let pricing = pricing(uuid::Uuid::new_v4().to_string());
        let runtime = Runtime {
            client: &client,
            store: &store,
            label: "fixture",
        };
        let proof = runtime.verify(&pricing).await.unwrap();
        assert_eq!(proof.subject, "fixture-user");
        assert!(Store::open(&path).is_err());
        let mut wrong = pricing.clone();
        if let ValuePricing::Subscription { model, .. } = &mut wrong {
            *model = "unavailable".into();
        }
        assert!(runtime.verify(&wrong).await.is_err());
        assert_eq!(client.calls.load(Ordering::SeqCst), 0);
        let mut value = serde_json::to_value(store.data.accounts.get("fixture").unwrap()).unwrap();
        value["credentials"]["scopes"] = serde_json::json!(["openid"]);
        store
            .data
            .accounts
            .insert("fixture".into(), serde_json::from_value(value).unwrap());
        assert!(
            Runtime {
                client: &client,
                store: &store,
                label: "fixture"
            }
            .verify(&pricing)
            .await
            .is_err()
        );
        drop(store);
        assert!(Store::open(&path).is_ok());
        std::fs::remove_dir_all(path).unwrap();
    }
}
