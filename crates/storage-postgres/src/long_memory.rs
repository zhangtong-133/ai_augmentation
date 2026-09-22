use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    long_memory::{LongMemoryStore, MemoryFact, validate},
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

const FIELDS: &str = "id,title,content,version,(extract(epoch FROM created_at)*1000)::bigint AS created_ms,(extract(epoch FROM updated_at)*1000)::bigint AS updated_ms";
#[allow(clippy::needless_pass_by_value)] // 直接用于查询结果的 map。
fn fact(row: PgRow) -> MemoryFact {
    MemoryFact {
        id: row.get::<Uuid, _>("id").to_string(),
        title: row.get("title"),
        content: row.get("content"),
        version: row.get("version"),
        created_at_unix_ms: row.get("created_ms"),
        updated_at_unix_ms: row.get("updated_ms"),
    }
}
fn uuid(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid id".into()))
}

impl PostgresStore {
    async fn fact_conflict(&self, owner: Uuid, id: Uuid) -> StorageError {
        match sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM memory_facts WHERE user_id=$1 AND id=$2)",
        )
        .bind(owner)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        {
            Ok(true) => StorageError::Conflict("memory version changed".into()),
            Ok(false) => StorageError::NotFound,
            Err(error) => map_error(error),
        }
    }
}

impl LongMemoryStore for PostgresStore {
    fn list_facts(
        &self,
        owner: &UserId,
        offset: i64,
    ) -> BoxFuture<'_, StorageResult<Vec<MemoryFact>>> {
        let owner = uuid(owner.as_str());
        Box::pin(async move {
            if !(0..=100).contains(&offset) {
                return Err(StorageError::InvalidData("invalid offset".into()));
            }
            sqlx::query(&format!("SELECT {FIELDS} FROM memory_facts WHERE user_id=$1 ORDER BY created_at DESC,id DESC LIMIT 20 OFFSET $2"))
                .bind(owner?).bind(offset).fetch_all(&self.pool).await.map(|rows| rows.into_iter().map(fact).collect()).map_err(map_error)
        })
    }
    fn create_fact(
        &self,
        owner: &UserId,
        title: &str,
        content: &str,
    ) -> BoxFuture<'_, StorageResult<MemoryFact>> {
        let owner = uuid(owner.as_str());
        let title = title.to_owned();
        let content = content.to_owned();
        Box::pin(async move {
            validate(&title, &content)?;
            let owner = owner?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            // 同一用户创建串行化，避免并发请求突破配额。
            let result = async {
                sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *tx).await.map_err(map_error)?;
                sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE").bind(owner).fetch_one(&mut *tx).await.map_err(map_error)?;
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_facts WHERE user_id=$1").bind(owner).fetch_one(&mut *tx).await.map_err(map_error)?;
                if count >= 100 { return Err(StorageError::Conflict("memory quota reached".into())); }
                sqlx::query(&format!("INSERT INTO memory_facts(id,user_id,title,content) VALUES($1,$2,$3,$4) RETURNING {FIELDS}"))
                    .bind(Uuid::new_v4()).bind(owner).bind(title).bind(content).fetch_one(&mut *tx).await.map(fact).map_err(map_error)
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
    fn update_fact(
        &self,
        owner: &UserId,
        id: &str,
        version: i64,
        title: &str,
        content: &str,
    ) -> BoxFuture<'_, StorageResult<MemoryFact>> {
        let owner = uuid(owner.as_str());
        let id = uuid(id);
        let title = title.to_owned();
        let content = content.to_owned();
        Box::pin(async move {
            validate(&title, &content)?;
            if version <= 0 {
                return Err(StorageError::InvalidData("invalid version".into()));
            }
            let (owner, id) = (owner?, id?);
            let row = sqlx::query(&format!("UPDATE memory_facts SET title=$4,content=$5,version=version+1,updated_at=clock_timestamp() WHERE user_id=$1 AND id=$2 AND version=$3 RETURNING {FIELDS}"))
                .bind(owner).bind(id).bind(version).bind(title).bind(content).fetch_optional(&self.pool).await.map_err(map_error)?;
            match row {
                Some(row) => Ok(fact(row)),
                None => Err(self.fact_conflict(owner, id).await),
            }
        })
    }
    fn delete_fact(
        &self,
        owner: &UserId,
        id: &str,
        version: i64,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let owner = uuid(owner.as_str());
        let id = uuid(id);
        Box::pin(async move {
            if version <= 0 {
                return Err(StorageError::InvalidData("invalid version".into()));
            }
            let (owner, id) = (owner?, id?);
            let result =
                sqlx::query("DELETE FROM memory_facts WHERE user_id=$1 AND id=$2 AND version=$3")
                    .bind(owner)
                    .bind(id)
                    .bind(version)
                    .execute(&self.pool)
                    .await
                    .map_err(map_error)?;
            if result.rows_affected() == 0 {
                return Err(self.fact_conflict(owner, id).await);
            }
            Ok(())
        })
    }
}
