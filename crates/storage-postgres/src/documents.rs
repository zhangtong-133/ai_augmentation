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
            // One INSERT atomically persists the original, metadata and all chunks.
            sqlx::query("INSERT INTO documents (id,user_id,title,source,tags,content_digest,markdown,chunks,created_at_unix_ms,source_type,original_pdf,original_html) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
                .bind(parse_id(&document.summary.id)?).bind(owner?)
                .bind(document.summary.title).bind(document.summary.source).bind(document.summary.tags)
                .bind(digest).bind(document.markdown).bind(document.chunks).bind(document.summary.created_at_unix_ms).bind(document.summary.source_type).bind(document.original_pdf).bind(document.original_html)
                .execute(&self.pool).await.map_err(|error| {
                    if error.as_database_error().is_some_and(sqlx::error::DatabaseError::is_unique_violation) {
                        StorageError::Conflict("document already exists".into())
                    } else { map_error(error) }
                })?;
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
            let row = sqlx::query("SELECT id,title,source,source_type,tags,created_at_unix_ms,cardinality(chunks) AS chunk_count,markdown,chunks,original_pdf,original_html FROM documents WHERE user_id=$1 AND id=$2")
                .bind(owner?).bind(id?).fetch_one(&self.pool).await.map_err(map_error)?;
            Ok(StoredDocument {
                summary: summary(&row),
                markdown: row.get("markdown"),
                original_pdf: row.get("original_pdf"),
                original_html: row.get("original_html"),
                chunks: row.get("chunks"),
            })
        })
    }
}
