use super::*;
use serde_json::json;

fn sources() -> Vec<AnswerSource> {
    vec![AnswerSource {
        id: 1,
        text: "唯一🙂证据".into(),
    }]
}

#[test]
fn backend_values_are_explicit_and_local_is_the_default() {
    assert!(AnswerBackend::default().is_local());
    for (backend, value, profile) in [
        (AnswerBackend::Local, "local", PROFILE),
        (AnswerBackend::Ollama, "ollama", OLLAMA_PROFILE),
    ] {
        assert_eq!(serde_json::to_value(backend).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<AnswerBackend>(json!(value)).unwrap(),
            backend
        );
        assert_eq!(backend.profile(), profile);
    }
    assert!(serde_json::from_value::<AnswerBackend>(json!("cloud")).is_err());
}

#[test]
fn local_preview_keeps_the_existing_serialized_contract_and_fingerprint() {
    let target = LocalTarget::new("http://127.0.0.1:11435", "fixture").unwrap();
    let legacy = preview(&target, "question", &sources()).unwrap();
    let explicit = preview_for(AnswerBackend::Local, &target, "question", &sources()).unwrap();
    assert_eq!(
        serde_json::to_vec(&legacy).unwrap(),
        serde_json::to_vec(&explicit).unwrap()
    );
    assert_eq!(legacy.backend(), AnswerBackend::Local);
    assert_eq!(legacy.profile(), PROFILE);
    assert_eq!(
        legacy.endpoint(),
        "http://127.0.0.1:11435/v1/chat/completions"
    );
    assert_eq!(
        legacy.fingerprint().unwrap(),
        "56a897cc6812d32b855d6acf52e7dfa5d855800adc5fc5f6bccd37deb998cc78"
    );
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("backend")
            .is_none()
    );
}

#[test]
fn native_preview_binds_one_request_with_shared_protocol_and_fixed_resources() {
    let target = LocalTarget::new("http://127.0.0.1:11435", "fixture").unwrap();
    let native = preview_for(AnswerBackend::Ollama, &target, "question", &sources()).unwrap();
    let legacy = preview(&target, "question", &sources()).unwrap();
    assert_eq!(native.backend(), AnswerBackend::Ollama);
    assert_eq!(native.profile(), OLLAMA_PROFILE);
    assert_eq!(native.endpoint(), "http://127.0.0.1:11435/api/chat");
    assert_eq!(native.body()["messages"], legacy.body()["messages"]);
    assert_eq!(
        native.body()["format"],
        *prepare("question", &sources()).unwrap().schema()
    );
    let serialized = serde_json::to_value(&native).unwrap();
    assert_eq!(
        serialized["protocol_sha256"],
        serde_json::to_value(&legacy).unwrap()["protocol_sha256"]
    );
    assert_eq!(native.body()["runner"], "llamacpp");
    assert_eq!(native.body()["stream"], true);
    assert_eq!(native.body()["think"], false);
    assert_eq!(native.body()["keep_alive"], 30);
    assert_eq!(native.body()["truncate"], false);
    assert_eq!(native.body()["shift"], false);
    assert_eq!(
        native.body()["options"],
        json!({"num_ctx":8192,"num_predict":2048,"temperature":0,"seed":0,
            "top_k":1,"top_p":1,"repeat_penalty":1,"presence_penalty":0,
            "frequency_penalty":0})
    );
    assert_ne!(native.fingerprint().unwrap(), legacy.fingerprint().unwrap());
    assert_ne!(
        native.fingerprint().unwrap(),
        preview_for(AnswerBackend::Ollama, &target, "question ", &sources())
            .unwrap()
            .fingerprint()
            .unwrap()
    );
    for changed in [
        LocalTarget::new("http://127.0.0.1:11436", "fixture").unwrap(),
        LocalTarget::new("http://127.0.0.1:11435", "other").unwrap(),
    ] {
        assert_ne!(
            native.fingerprint().unwrap(),
            preview_for(AnswerBackend::Ollama, &changed, "question", &sources())
                .unwrap()
                .fingerprint()
                .unwrap()
        );
    }
    let oversized: Vec<_> = (1..=5)
        .map(|id| AnswerSource {
            id,
            text: "🙂".repeat(1000),
        })
        .collect();
    assert!(prepare("question", &oversized).is_ok());
    assert!(preview_for(AnswerBackend::Ollama, &target, "question", &oversized).is_err());
}
