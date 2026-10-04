use super::*;
use personal_ai_llm::{AnswerCitation, AnswerProvider, AnswerSource, ModelAnswer};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct PausedAnswer {
    started: tokio::sync::Notify,
    resume: tokio::sync::Notify,
    calls: AtomicUsize,
    insufficient: bool,
}
impl AnswerProvider for PausedAnswer {
    fn answer(
        &self,
        _: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, personal_ai_llm::LlmResult<ModelAnswer>> {
        let id = sources
            .iter()
            .find(|s| s.text == "verified evidence")
            .unwrap()
            .id;
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.resume.notified().await;
            Ok(ModelAnswer {
                answer: if self.insufficient {
                    String::new()
                } else {
                    "private generated answer".into()
                },
                citations: if self.insufficient {
                    vec![]
                } else {
                    vec![AnswerCitation {
                        id,
                        quote: "verified evidence".into(),
                    }]
                },
                insufficient_evidence: self.insufficient,
            })
        })
    }
}
#[tokio::test]
async fn answers_discard_inflight_content_after_source_or_session_changes() {
    for mutation in [
        "delete",
        "text",
        "title",
        "source",
        "owner",
        "logout",
        "insufficient_logout",
        "uncited",
    ] {
        let (mut state, store, _, owner, cookie, document) = retrieval_fixture().await;
        let mut changed_id = document.summary.id.clone();
        if mutation == "uncited" {
            let mut extra = document.clone();
            extra.summary.id = Uuid::new_v4().to_string();
            extra.chunks = vec!["uncited evidence".into()];
            store
                .insert_document(&owner, "extra", &extra)
                .await
                .unwrap();
            assert!(
                state
                    .indexing
                    .as_ref()
                    .unwrap()
                    .indexer
                    .index_batch(&owner, &extra, 0)
                    .await
                    .is_ok()
            );
            changed_id = extra.summary.id;
        }
        let provider = Arc::new(PausedAnswer {
            insufficient: mutation == "insufficient_logout",
            ..PausedAnswer::default()
        });
        state.answering = Some(provider.clone());
        let app = router(state);
        let request = tokio::spawn(async move {
            auth_request(
                app,
                "POST",
                "/api/knowledge/answer",
                Some(&cookie),
                r#"{"query":"question"}"#,
                true,
            )
            .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            provider.started.notified(),
        )
        .await
        .unwrap();
        match mutation {
            "logout" | "insufficient_logout" => store.sessions.lock().unwrap().clear(),
            "delete" | "uncited" => {
                store.documents.lock().unwrap().remove(&changed_id);
            }
            _ => {
                let mut documents = store.documents.lock().unwrap();
                let (user, _, saved) = documents.get_mut(&changed_id).unwrap();
                match mutation {
                    "text" => saved.chunks[0] = "new evidence".into(),
                    "title" => saved.summary.title = "new title".into(),
                    "source" => saved.summary.source = "new.md".into(),
                    "owner" => *user = Uuid::new_v4().to_string(),
                    _ => unreachable!(),
                }
            }
        }
        provider.resume.notify_one();
        let response = request.await.unwrap();
        assert_eq!(
            response.status(),
            if mutation.contains("logout") {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::CONFLICT
            }
        );
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"error":{"code":if mutation.contains("logout") {"unauthorized"} else {"answer_evidence_changed"}}})
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn answers_fail_closed_on_pre_send_and_post_generation_source_read_failures() {
    for fail_at in [2, 3] {
        let (mut state, store, _, _, cookie, _) = retrieval_fixture().await;
        store.fail_document_read_at.store(fail_at, Ordering::SeqCst);
        let provider = Arc::new(AnswerFixture::default());
        state.answering = Some(provider.clone());
        let response = auth_request(
            router(state),
            "POST",
            "/api/knowledge/answer",
            Some(&cookie),
            r#"{"query":"question"}"#,
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"error":{"code":"answer_evidence_unavailable"}})
        );
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            usize::from(fail_at == 3)
        );
    }
}
