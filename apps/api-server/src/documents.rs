use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use personal_ai_domain::DocumentId;
use personal_ai_knowledge::{chunk_text, markdown_text};
use personal_ai_pdf_poppler::{MAX_PDF_BYTES, PdfError};
use personal_ai_storage::{
    StorageError,
    documents::{DocumentSummary, StoredDocument},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/documents", get(list).post(import))
        .route("/api/documents/{id}", get(detail))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    title: String,
    markdown: Option<String>,
    pdf_base64: Option<String>,
    #[serde(default)]
    source: String,
    #[serde(default)]
    tags: Vec<String>,
}
fn document_error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "document_not_found"),
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "duplicate_document"),
        other => other.into(),
    }
}
async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<Import>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|e| ApiError(e.status(), "invalid_json"))?;
    let title = input.title.trim().to_owned();
    if title.is_empty()
        || title.chars().count() > 200
        || title.chars().any(char::is_control)
        || input.source.len() > 1024
        || input.source.chars().any(char::is_control)
        || input.tags.len() > 20
        || input.tags.iter().any(|t| {
            t.trim().is_empty() || t.chars().count() > 40 || t.chars().any(char::is_control)
        })
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_document"));
    }
    let (content, text, original_pdf, source_type, digest) = prepare_content(&input).await?;
    let id = Uuid::new_v4().to_string();
    let chunks: Vec<_> = chunk_text(&DocumentId::new(&id), &text, 1000)
        .into_iter()
        .map(|c| c.content)
        .collect();
    if chunks.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "empty_document"));
    }
    let created_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|t| i64::try_from(t.as_millis()).ok())
        .ok_or(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "clock_error"))?;
    let chunk_count = i32::try_from(chunks.len())
        .map_err(|_| ApiError(StatusCode::PAYLOAD_TOO_LARGE, "document_too_large"))?;
    let mut tags: Vec<_> = input
        .tags
        .into_iter()
        .map(|t| t.trim().to_owned())
        .collect();
    tags.sort();
    tags.dedup();
    let document = StoredDocument {
        summary: DocumentSummary {
            id,
            title,
            source: input.source,
            source_type,
            tags,
            created_at_unix_ms,
            chunk_count,
        },
        markdown: content,
        original_pdf,
        chunks,
    };
    state
        .documents
        .insert_document(&user.id, &digest, &document)
        .await
        .map_err(document_error)?;
    Ok((
        StatusCode::CREATED,
        [
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (
                header::LOCATION,
                format!("/api/documents/{}", document.summary.id),
            ),
        ],
        Json(document.summary),
    ))
}
// Keep Markdown's existing digest and response field for compatibility.
async fn prepare_content(
    input: &Import,
) -> Result<(String, String, Option<Vec<u8>>, String, String), ApiError> {
    match (&input.markdown, &input.pdf_base64) {
        (Some(markdown), None) => {
            if markdown.len() > 256 * 1024 {
                return Err(ApiError(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "document_too_large",
                ));
            }
            if markdown.trim().is_empty() || markdown.contains('\0') {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_document"));
            }
            Ok((
                markdown.clone(),
                markdown_text(&markdown.replace("\r\n", "\n")),
                None,
                "markdown".into(),
                format!("{:x}", Sha256::digest(markdown.as_bytes())),
            ))
        }
        (None, Some(encoded)) => {
            if encoded.len() > MAX_PDF_BYTES.div_ceil(3) * 4 {
                return Err(ApiError(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "document_too_large",
                ));
            }
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_pdf"))?;
            let text =
                personal_ai_pdf_poppler::extract(&bytes)
                    .await
                    .map_err(|error| match error {
                        PdfError::Invalid => ApiError(StatusCode::BAD_REQUEST, "invalid_pdf"),
                        PdfError::TooLarge => {
                            ApiError(StatusCode::PAYLOAD_TOO_LARGE, "document_too_large")
                        }
                        PdfError::Empty => {
                            ApiError(StatusCode::UNPROCESSABLE_ENTITY, "pdf_no_text")
                        }
                        PdfError::Unavailable => {
                            ApiError(StatusCode::SERVICE_UNAVAILABLE, "pdf_unavailable")
                        }
                        PdfError::Busy => ApiError(StatusCode::SERVICE_UNAVAILABLE, "pdf_busy"),
                        PdfError::Timeout => {
                            ApiError(StatusCode::UNPROCESSABLE_ENTITY, "pdf_timeout")
                        }
                    })?;
            let digest = format!("pdf:{:x}", Sha256::digest(&bytes));
            Ok((text.clone(), text, Some(bytes), "pdf".into(), digest))
        }
        _ => Err(ApiError(StatusCode::BAD_REQUEST, "invalid_document")),
    }
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    offset: u32,
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let Query(page) = query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_page"))?;
    if page.offset > 100_000 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_page"));
    }
    let documents = state
        .documents
        .list_documents(&user.id, page.offset)
        .await
        .map_err(document_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(documents)))
}
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_document_id"))?;
    let document = state
        .documents
        .get_document(&user.id, &id.to_string())
        .await
        .map_err(document_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(document)))
}
