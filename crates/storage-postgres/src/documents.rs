use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    documents::{DocumentStats, DocumentStore, DocumentSummary, StoredDocument},
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

fn summary(row: &PgRow) -> DocumentSummary {
    DocumentSummary {
        id: row.get::<Uuid, _>("id").to_string(),
        title: row.get("title"),
        source: row.get("source"),
        source_type: row.get("source_type"),
        tags: row.get("tags"),
        created_at_unix_ms: row.get("created_at_unix_ms"),
        chunk_count: row.get("chunk_count"),
    }
}

fn parse_id(id: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(id).map_err(|_| StorageError::InvalidData("invalid id".into()))
}

impl DocumentStore for PostgresStore {
    fn document_stats(
        &self,
        owner: &UserId,
        start_ms: i64,
        end_ms: i64,
    ) -> BoxFuture<'_, StorageResult<DocumentStats>> {
        let owner = parse_id(owner.as_str());
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT COUNT(*) AS total_documents, \
                 COALESCE(SUM(cardinality(chunks)), 0)::bigint AS total_chunks, \
                 COUNT(*) FILTER (WHERE created_at_unix_ms >= $2 AND created_at_unix_ms < $3) AS imported_today \
                 FROM documents WHERE user_id=$1",
            )
            .bind(owner?).bind(start_ms).bind(end_ms)
            .fetch_one(&self.pool).await.map_err(map_error)?;
            Ok(DocumentStats {
                total_documents: row.get("total_documents"),
                total_chunks: row.get("total_chunks"),
                imported_today: row.get("imported_today"),
            })
        })
    }
    fn insert_document(
        &self,
        owner: &UserId,
        digest: &str,
        document: &StoredDocument,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let owner = parse_id(owner.as_str());
        let digest = digest.to_owned();
        let document = document.clone();
        Box::pin(async move {
            let owner = owner?;
            let id = parse_id(&document.summary.id)?;
            let original = original(&document)?;
            // 每次尝试使用独立对象键，避免失败清理或重复导入覆盖已提交原文。
            let key = self
                .objects
                .as_ref()
                .map(|_| format!("users/{owner}/documents/{id}/{}", Uuid::new_v4()));
            let external = key.is_some();
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            // 先获得唯一约束，再上传；重复导入不会产生对象。
            sqlx::query("INSERT INTO documents (id,user_id,title,source,tags,content_digest,markdown,chunks,created_at_unix_ms,source_type,original_pdf,original_html,original_object_key) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
                .bind(id).bind(owner)
                .bind(&document.summary.title).bind(&document.summary.source).bind(&document.summary.tags)
                .bind(digest).bind(&document.markdown).bind(&document.chunks).bind(document.summary.created_at_unix_ms).bind(&document.summary.source_type)
                .bind(if external { None } else { document.original_pdf.as_deref() })
                .bind(if external { None } else { document.original_html.as_deref() }).bind(&key)
                .execute(&mut *tx).await.map_err(|error| {
                    if error.as_database_error().is_some_and(sqlx::error::DatabaseError::is_unique_violation) {
                        StorageError::Conflict("document already exists".into())
                    } else { map_error(error) }
                })?;
            if let (Some(objects), Some(key)) = (&self.objects, &key) {
                objects.put(key, original.0, Some(original.1)).await?;
            }
            // 提交失败可能只表示回执丢失，不能删除可能已被数据库引用的对象。
            tx.commit().await.map_err(map_error)?;
            Ok(())
        })
    }
    fn list_documents(
        &self,
        owner: &UserId,
        offset: u32,
    ) -> BoxFuture<'_, StorageResult<Vec<DocumentSummary>>> {
        let owner = parse_id(owner.as_str());
        Box::pin(async move {
            let rows = sqlx::query("SELECT id,title,source,source_type,tags,created_at_unix_ms,cardinality(chunks) AS chunk_count FROM documents WHERE user_id=$1 ORDER BY created_at_unix_ms DESC,id DESC LIMIT 20 OFFSET $2")
                .bind(owner?).bind(i64::from(offset)).fetch_all(&self.pool).await.map_err(map_error)?;
            Ok(rows.iter().map(summary).collect())
        })
    }
    fn get_document(
        &self,
        owner: &UserId,
        id: &str,
    ) -> BoxFuture<'_, StorageResult<StoredDocument>> {
        let owner = parse_id(owner.as_str());
        let id = parse_id(id);
        Box::pin(async move {
            let row = sqlx::query("SELECT id,title,source,source_type,tags,created_at_unix_ms,cardinality(chunks) AS chunk_count,markdown,chunks,original_pdf,original_html,original_object_key FROM documents WHERE user_id=$1 AND id=$2")
                .bind(owner?).bind(id?).fetch_one(&self.pool).await.map_err(map_error)?;
            let mut document = StoredDocument {
                summary: summary(&row),
                markdown: row.get("markdown"),
                original_pdf: row.get("original_pdf"),
                original_html: row.get("original_html"),
                chunks: row.get("chunks"),
            };
            if let Some(key) = row.get::<Option<String>, _>("original_object_key") {
                let objects = self.objects.as_ref().ok_or_else(|| {
                    StorageError::Unavailable("object store is not configured".into())
                })?;
                let bytes = objects.get(&key).await.map_err(|_| {
                    StorageError::Unavailable("document original unavailable".into())
                })?;
                match document.summary.source_type.as_str() {
                    "pdf" => document.original_pdf = Some(bytes),
                    "web_page" => document.original_html = Some(original_text(bytes)?),
                    "markdown" => document.markdown = original_text(bytes)?,
                    _ => return Err(StorageError::InvalidData("invalid source type".into())),
                }
            }
            Ok(document)
        })
    }
}

fn original_text(bytes: Vec<u8>) -> StorageResult<String> {
    String::from_utf8(bytes).map_err(|_| StorageError::InvalidData("invalid original text".into()))
}

fn original(document: &StoredDocument) -> StorageResult<(&[u8], &'static str)> {
    let value = match document.summary.source_type.as_str() {
        "markdown" if document.original_pdf.is_none() && document.original_html.is_none() => {
            Some((
                document.markdown.as_bytes(),
                "text/markdown; charset=utf-8",
                256 * 1024,
            ))
        }
        "pdf" if document.original_html.is_none() => document
            .original_pdf
            .as_deref()
            .map(|v| (v, "application/pdf", 5 * 1024 * 1024)),
        "web_page" if document.original_pdf.is_none() => document
            .original_html
            .as_deref()
            .map(|v| (v.as_bytes(), "text/html; charset=utf-8", 1024 * 1024)),
        _ => None,
    };
    match value {
        Some((bytes, content_type, limit)) if !bytes.is_empty() && bytes.len() <= limit => {
            Ok((bytes, content_type))
        }
        _ => Err(StorageError::InvalidData(
            "invalid document original".into(),
        )),
    }
}
