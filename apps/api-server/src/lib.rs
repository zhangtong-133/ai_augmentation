mod answering;
pub use answering::answering_from_env;
mod index_jobs;
mod indexing;
mod object_storage;
mod retrieval;
mod tool_execution;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::JsonRejection},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
pub use indexing::{Indexing, indexing_from_env};
pub use object_storage::object_storage_from_env;
use personal_ai_domain::{User, UserId};
use personal_ai_storage::{MetadataStore, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{net::SocketAddr, sync::Arc};
use subtle::ConstantTimeEq;
use uuid::Uuid;
mod auth;
mod documents;
mod overview;
pub use auth::AuthConfig;

pub struct Config {
    pub address: SocketAddr,
    pub database_url: String,
    pub api_token: String,
}

impl Config {
    /// 读取配置，不输出密钥。
    ///
    /// # Errors
    /// 凭据缺失或监听地址格式错误时返回错误。
    pub fn from_env() -> Result<Self, String> {
        Self::load(|key| std::env::var(key).ok())
    }

    fn load(env: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let host = env("API_HOST").unwrap_or_else(|| "127.0.0.1".into());
        let port = env("API_PORT").unwrap_or_else(|| "8080".into());
        let address = format!("{host}:{port}")
            .parse()
            .map_err(|_| "invalid API_HOST/API_PORT")?;
        let database_url = env("DATABASE_URL")
            .filter(|s| !s.trim().is_empty())
            .ok_or("DATABASE_URL is required")?;
        let api_token = env("API_AUTH_TOKEN")
            .filter(|s| s.len() >= 32 && s.bytes().all(|b| b.is_ascii_graphic()))
            .ok_or("API_AUTH_TOKEN must contain at least 32 visible ASCII characters")?;
        Ok(Self {
            address,
            database_url,
            api_token,
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub answering: Option<Arc<dyn personal_ai_llm::AnswerProvider>>,
    pub indexing: Option<Arc<Indexing>>,
    pub web_importer: Arc<dyn personal_ai_knowledge::web::WebImporter>,
    pub documents: Arc<dyn personal_ai_storage::documents::DocumentStore>,
    pub store: Arc<dyn MetadataStore>,
    pub api_token: Arc<str>,
    pub auth: Arc<AuthConfig>,
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/users", post(create_user))
        .route("/api/users/{id}", get(get_user))
        .route("/api/users/{id}/password", post(auth::set_password))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
    Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .merge(documents::routes())
        .merge(indexing::routes())
        .merge(retrieval::routes())
        .merge(answering::routes())
        .merge(index_jobs::routes())
        .merge(tool_execution::routes(&state))
        .route("/api/overview", get(overview::get))
        .route("/healthz", get(health))
        .route("/api/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/api/readyz", get(ready))
        .merge(protected)
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "not_found") })
        .method_not_allowed_fallback(|| async {
            ApiError(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed")
        })
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(log_request))
        .with_state(state)
}

async fn log_request(request: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let method = request.method().clone();
    let response = next.run(request).await;
    tracing::info!(
        method = %method,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        "HTTP request completed"
    );
    response
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "service": "api-server"}))
}

async fn ready(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    state.store.health().await.map_err(ApiError::from)?;
    Ok(Json(json!({"status": "ready"})))
}

async fn authorize(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !provided.is_some_and(|token| bool::from(token.as_bytes().ct_eq(state.api_token.as_bytes())))
    {
        return (
            [(header::WWW_AUTHENTICATE, "Bearer")],
            ApiError(StatusCode::UNAUTHORIZED, "unauthorized"),
        )
            .into_response();
    }
    next.run(request).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUser {
    email: String,
    display_name: String,
}

#[derive(Serialize)]
struct UserResponse {
    id: String,
    email: String,
    display_name: String,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id.to_string(),
            email: user.email,
            display_name: user.display_name,
        }
    }
}

async fn create_user(
    State(state): State<AppState>,
    payload: Result<Json<CreateUser>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(input) = payload.map_err(|error| ApiError(error.status(), "invalid_json"))?;
    let email = input.email.trim().to_lowercase();
    let display_name = input.display_name.trim().to_owned();
    let valid_email = email.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && !domain.is_empty() && !domain.contains('@')
    });
    if email.len() > 254
        || !valid_email
        || email.chars().any(char::is_whitespace)
        || email.chars().any(char::is_control)
        || display_name.is_empty()
        || display_name.chars().count() > 100
        || display_name.chars().any(char::is_control)
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_user"));
    }
    let user = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email,
        display_name,
    };
    state.store.save_user(&user).await?;
    let location = format!("/api/users/{}", user.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(UserResponse::from(user)),
    ))
}

async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<UserResponse>, ApiError> {
    let id =
        Uuid::parse_str(&id).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_user_id"))?;
    let user = state.store.get_user(&UserId::new(id.to_string())).await?;
    Ok(Json(user.into()))
}

struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": {"code": self.1}}))).into_response()
    }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound => Self(StatusCode::NOT_FOUND, "user_not_found"),
            StorageError::Conflict(_) => Self(StatusCode::CONFLICT, "user_exists"),
            StorageError::InvalidData(_) => Self(StatusCode::BAD_REQUEST, "invalid_user"),
            StorageError::Unavailable(_) => {
                tracing::warn!("storage unavailable");
                Self(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable")
            }
        }
    }
}

#[cfg(test)]
mod tests;
