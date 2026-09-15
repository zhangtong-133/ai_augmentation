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
use personal_ai_knowledge::{chunk_text, markdown_text, web::WebImportError};
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
    #[serde(default)]
    title: String,
    url: Option<String>,
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
    if (title.is_empty() && input.url.is_none())
        || title.chars().count() > 200
        || title.chars().any(char::is_control)
        || (input.url.is_some() && !input.source.is_empty())
        || input.source.len() > 1024
        || input.source.chars().any(char::is_control)
        || input.tags.len() > 20
        || input.tags.iter().any(|t| {
            t.trim().is_empty() || t.chars().count() > 40 || t.chars().any(char::is_control)
        })
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_document"));
    }
    let prepared = prepare_content(&state, &input).await?;
    let title = if title.is_empty() {
        prepared.title.clone().unwrap_or_default()
    } else {
        title
    };
    let id = Uuid::new_v4().to_string();
    let chunks: Vec<_> = chunk_text(&DocumentId::new(&id), &prepared.text, 1000)
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
            source: prepared.source.unwrap_or(input.source),
            source_type: prepared.source_type,
            tags,
            created_at_unix_ms,
            chunk_count,
        },
        markdown: prepared.content,
        original_pdf: prepared.original_pdf,
        original_html: prepared.original_html,
        chunks,
    };
    state
        .documents
        .insert_document(&user.id, &prepared.digest, &document)
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
struct PreparedContent {
    content: String,
    text: String,
    original_pdf: Option<Vec<u8>>,
    original_html: Option<String>,
    source_type: String,
    digest: String,
    source: Option<String>,
    title: Option<String>,
}

// Keep Markdown's existing digest and response field for compatibility.
async fn prepare_content(state: &AppState, input: &Import) -> Result<PreparedContent, ApiError> {
    match (&input.markdown, &input.pdf_base64, &input.url) {
        (Some(markdown), None, None) => {
            if markdown.len() > 256 * 1024 {
                return Err(ApiError(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "document_too_large",
                ));
            }
            if markdown.trim().is_empty() || markdown.contains('\0') {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_document"));
            }
            Ok(PreparedContent {
                content: markdown.clone(),
                text: markdown_text(&markdown.replace("\r\n", "\n")),
                original_pdf: None,
                original_html: None,
                source_type: "markdown".into(),
                digest: format!("{:x}", Sha256::digest(markdown.as_bytes())),
                source: None,
                title: None,
            })
        }
        (None, Some(encoded), None) => {
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
            Ok(PreparedContent {
                content: text.clone(),
                text,
                original_pdf: Some(bytes),
                original_html: None,
                source_type: "pdf".into(),
                digest,
                source: None,
                title: None,
            })
        }
        (None, None, Some(url)) => {
            let page = state.web_importer.import(url).await.map_err(web_error)?;
            let title = if page.title.is_empty() {
                page.source.chars().take(200).collect()
            } else {
                page.title
            };
            let digest = format!("web:{:x}", Sha256::digest(page.html.as_bytes()));
            Ok(PreparedContent {
                content: page.text.clone(),
                text: page.text,
                original_pdf: None,
                original_html: Some(page.html),
                source_type: "web_page".into(),
                digest,
                source: Some(page.source),
                title: Some(title),
            })
        }
        _ => Err(ApiError(StatusCode::BAD_REQUEST, "invalid_document")),
    }
}
fn web_error(error: WebImportError) -> ApiError {
    match error {
        WebImportError::InvalidUrl => ApiError(StatusCode::BAD_REQUEST, "invalid_url"),
        WebImportError::Blocked => ApiError(StatusCode::BAD_REQUEST, "url_blocked"),
        WebImportError::TooLarge => ApiError(StatusCode::PAYLOAD_TOO_LARGE, "web_too_large"),
        WebImportError::Unsupported => {
            ApiError(StatusCode::UNPROCESSABLE_ENTITY, "web_unsupported")
        }
        WebImportError::Empty => ApiError(StatusCode::UNPROCESSABLE_ENTITY, "web_empty"),
        WebImportError::Timeout => ApiError(StatusCode::GATEWAY_TIMEOUT, "web_timeout"),
        WebImportError::Busy => ApiError(StatusCode::SERVICE_UNAVAILABLE, "web_busy"),
        WebImportError::Redirect => ApiError(StatusCode::UNPROCESSABLE_ENTITY, "web_redirect"),
        WebImportError::Unavailable => ApiError(StatusCode::BAD_GATEWAY, "web_unavailable"),
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
