use axum::{
    Router,
    extract::{RawQuery, State},
    http::StatusCode,
    routing::get,
};
use personal_ai_llm_openai::chatgpt::{CodeExchange, Error, PendingLogin, Result};
use std::sync::Arc;
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
};

type Transaction = (PendingLogin, oneshot::Sender<Result<CodeExchange>>);
#[derive(Clone)]
struct Callback(Arc<Mutex<Option<Transaction>>>);
async fn callback(
    State(state): State<Callback>,
    RawQuery(query): RawQuery,
) -> (StatusCode, [(String, String); 2], &'static str) {
    let headers = [
        ("Cache-Control".into(), "no-store".into()),
        ("Referrer-Policy".into(), "no-referrer".into()),
    ];
    let mut transaction = state.0.lock().await;
    let Some((pending, _)) = transaction.as_ref() else {
        return (StatusCode::CONFLICT, headers, "Callback already received.");
    };
    let Some(query) = query.filter(|q| pending.matches_state(q)) else {
        return (
            StatusCode::BAD_REQUEST,
            headers,
            "Invalid callback. Continue the original sign-in in your terminal.",
        );
    };
    if let Some((pending, sender)) = transaction.take() {
        let _ = sender.send(pending.callback(&query));
    }
    (
        StatusCode::OK,
        headers,
        "Callback received. Return to your terminal for the verified sign-in result.",
    )
}
pub async fn receive(listener: TcpListener, pending: PendingLogin) -> Result<CodeExchange> {
    let (sender, receiver) = oneshot::channel();
    let router = Router::new()
        .route("/auth/callback", get(callback))
        .with_state(Callback(Arc::new(Mutex::new(Some((pending, sender))))));
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let result = tokio::time::timeout(std::time::Duration::from_secs(300), receiver).await;
    server.abort();
    let _ = server.await;
    result
        .map_err(|_| Error("sign-in timed out"))?
        .map_err(|_| Error("callback listener stopped"))?
}

#[cfg(test)]
mod tests {
    use super::{Callback, callback};
    use axum::{
        extract::{RawQuery, State},
        http::StatusCode,
    };
    use personal_ai_llm_openai::chatgpt::PendingLogin;
    use std::sync::Arc;
    use tokio::sync::{Mutex, oneshot};
    use uuid::Uuid;
    #[tokio::test]
    async fn rejects_foreign_state_and_consumes_callback_only_once() {
        let (pending, url) =
            PendingLogin::new(&Uuid::new_v4().urn().to_string(), 1234, None).unwrap();
        let state = url
            .split("state=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let query = format!("state={state}&code=test&client_id=oaiapp_test");
        let (sender, receiver) = oneshot::channel();
        let callback_state = Callback(Arc::new(Mutex::new(Some((pending, sender)))));
        assert_eq!(
            callback(
                State(callback_state.clone()),
                RawQuery(Some("state=wrong".into()))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            callback(State(callback_state.clone()), RawQuery(Some(query.clone())))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            receiver.await.unwrap().unwrap().registration.client_id,
            "oaiapp_test"
        );
        assert_eq!(
            callback(State(callback_state), RawQuery(Some(query)))
                .await
                .0,
            StatusCode::CONFLICT
        );
    }
}
