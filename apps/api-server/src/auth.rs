use super::{
    ApiError, AppState, Deserialize, IntoResponse, Json, JsonRejection, Path, State, StatusCode,
    StorageError, UserId, UserResponse, Uuid, header, json,
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::http::HeaderMap;
use sha2::{Digest, Sha256};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

pub struct AuthConfig {
    pub secure_cookie: bool,
    attempts: Mutex<(Instant, u32)>,
}
impl AuthConfig {
    #[must_use]
    pub fn new(secure_cookie: bool) -> Self {
        Self {
            secure_cookie,
            attempts: Mutex::new((Instant::now(), 0)),
        }
    }
}
pub(super) fn mutation_guard(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers
        .get("x-requested-with")
        .and_then(|h| h.to_str().ok())
        != Some("personal-ai")
    {
        return Err(ApiError(StatusCode::FORBIDDEN, "csrf_rejected"));
    }
    Ok(())
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn cookie_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| {
            h.split(';')
                .map(str::trim)
                .find_map(|p| p.strip_prefix("personal_ai_session_v2="))
        })
        .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))
}
fn cookie(value: &str, secure: bool, age: u32) -> String {
    format!(
        "personal_ai_session_v2={value}; HttpOnly; SameSite=Strict; Path=/api; Max-Age={age}{}",
        if secure { "; Secure" } else { "" }
    )
}
pub(super) async fn current_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<personal_ai_domain::User, ApiError> {
    state
        .store
        .session_user(&digest(cookie_token(headers)?))
        .await
        .map_err(session_error)
}

fn session_error(error: StorageError) -> ApiError {
    if matches!(error, StorageError::NotFound) {
        ApiError(StatusCode::UNAUTHORIZED, "unauthorized")
    } else {
        error.into()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Login {
    email: String,
    password: String,
}
pub(super) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Login>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    mutation_guard(&headers)?;
    {
        let mut attempts = state
            .auth
            .attempts
            .lock()
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "auth_unavailable"))?;
        if attempts.0.elapsed() >= Duration::from_mins(1) {
            *attempts = (Instant::now(), 0);
        }
        if attempts.1 >= 20 {
            return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
        }
        attempts.1 += 1;
    }
    let Json(input) = body.map_err(|e| ApiError(e.status(), "invalid_json"))?;
    if input.password.len() > 256 || input.email.len() > 254 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_credentials"));
    }
    let credentials = match state
        .store
        .password_hash(&input.email.trim().to_lowercase())
        .await
    {
        Ok(value) => Some(value),
        Err(StorageError::NotFound) => None,
        Err(e) => return Err(e.into()),
    };
    let hash = credentials.as_ref().map(|(_, hash)| hash.clone());
    let valid = tokio::task::spawn_blocking(move || {
        if let Some(hash) = hash {
            PasswordHash::new(&hash).is_ok_and(|hash| {
                Argon2::default()
                    .verify_password(input.password.as_bytes(), &hash)
                    .is_ok()
            })
        } else {
            let salt = SaltString::encode_b64(Uuid::new_v4().as_bytes()).expect("valid salt");
            let _ = Argon2::default().hash_password(input.password.as_bytes(), &salt);
            false
        }
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "auth_unavailable"))?;
    if !valid {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "invalid_credentials"));
    }
    let (id, hash) =
        credentials.ok_or(ApiError(StatusCode::UNAUTHORIZED, "invalid_credentials"))?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    state
        .store
        .create_session(&id, &digest(&token), &hash)
        .await
        .map_err(session_error)?;
    Ok((
        [
            (
                header::SET_COOKIE,
                cookie(&token, state.auth.secure_cookie, 28800),
            ),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        Json(json!({"status":"authenticated"})),
    ))
}
pub(super) async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = state
        .store
        .session_user(&digest(cookie_token(&headers)?))
        .await
        .map_err(session_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(UserResponse::from(user)),
    ))
}
pub(super) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    mutation_guard(&headers)?;
    if let Ok(token) = cookie_token(&headers) {
        state.store.delete_session(&digest(token)).await?;
    }
    Ok((
        [
            (header::SET_COOKIE, cookie("", state.auth.secure_cookie, 0)),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        Json(json!({"status":"signed_out"})),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PasswordInput {
    password: String,
}
pub(super) async fn set_password(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<PasswordInput>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let id =
        Uuid::parse_str(&id).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_user_id"))?;
    let Json(input) = body.map_err(|e| ApiError(e.status(), "invalid_json"))?;
    if input.password.len() < 12 || input.password.len() > 256 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "password_length"));
    }
    let hash = tokio::task::spawn_blocking(move || {
        let salt = SaltString::encode_b64(Uuid::new_v4().as_bytes()).expect("valid salt");
        Argon2::default()
            .hash_password(input.password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "auth_unavailable"))?
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "auth_unavailable"))?;
    state
        .store
        .set_password(&UserId::new(id.to_string()), &hash)
        .await?;
    Ok(Json(json!({"status":"password_updated"})))
}
