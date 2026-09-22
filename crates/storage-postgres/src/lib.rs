//! `PostgreSQL` 适配器，厂商特有类型不进入存储接口。
mod conversations;
mod documents;
mod index_jobs;
mod long_memory;
mod messages;
mod originals;
pub use originals::MaintenanceReport;
use personal_ai_domain::{User, UserId};
use personal_ai_storage::{BoxFuture, MetadataStore, StorageError, StorageResult};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

pub struct PostgresStore {
    pool: PgPool,
    objects: Option<Arc<dyn personal_ai_storage::ObjectStorage>>,
}

impl PostgresStore {
    /// 为新导入启用外部原文存储；旧记录继续从数据库读取。
    #[must_use]
    pub fn with_object_storage(
        mut self,
        objects: Arc<dyn personal_ai_storage::ObjectStorage>,
    ) -> Self {
        self.objects = Some(objects);
        self
    }

    /// 连接数据库并执行嵌入的版本化迁移。
    ///
    /// # Errors
    /// 数据库不可用或迁移失败时，返回不暴露内部细节的错误。
    pub async fn connect(url: &str) -> StorageResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await
            .map_err(map_error)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|_| StorageError::Unavailable("migration failed".into()))?;
        Ok(Self {
            pool,
            objects: None,
        })
    }
}

#[allow(clippy::needless_pass_by_value)] // 直接用于 Result::map_err。
fn map_error(error: sqlx::Error) -> StorageError {
    match error {
        sqlx::Error::RowNotFound => StorageError::NotFound,
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            StorageError::Conflict("user already exists".into())
        }
        _ => StorageError::Unavailable("database operation failed".into()),
    }
}

impl MetadataStore for PostgresStore {
    fn password_hash(&self, email: &str) -> BoxFuture<'_, StorageResult<(UserId, String)>> {
        let email = email.to_owned();
        Box::pin(async move {
            let row = sqlx::query("SELECT id, password_hash FROM users WHERE lower(email) = lower($1) AND password_hash IS NOT NULL")
                .bind(email).fetch_one(&self.pool).await.map_err(map_error)?;
            Ok((
                UserId::new(row.get::<Uuid, _>("id").to_string()),
                row.get("password_hash"),
            ))
        })
    }

    fn set_password(&self, id: &UserId, hash: &str) -> BoxFuture<'_, StorageResult<()>> {
        let id = Uuid::parse_str(id.as_str());
        let hash = hash.to_owned();
        Box::pin(async move {
            let id = id.map_err(|_| StorageError::InvalidData("invalid id".into()))?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result =
                sqlx::query("UPDATE users SET password_hash=$2, updated_at=NOW() WHERE id=$1")
                    .bind(id)
                    .bind(hash)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_error)?;
            if result.rows_affected() == 0 {
                return Err(StorageError::NotFound);
            }
            sqlx::query("DELETE FROM sessions WHERE user_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            tx.commit().await.map_err(map_error)
        })
    }

    fn create_session(
        &self,
        id: &UserId,
        digest: &str,
        credential_hash: &str,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let id = Uuid::parse_str(id.as_str());
        let digest = digest.to_owned();
        let credential_hash = credential_hash.to_owned();
        Box::pin(async move {
            let id = id.map_err(|_| StorageError::InvalidData("invalid id".into()))?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            // 与密码重置操作串行执行，并验证凭据仍然有效。
            sqlx::query("SELECT id FROM users WHERE id=$1 AND password_hash=$2 FOR UPDATE")
                .bind(id)
                .bind(credential_hash)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("DELETE FROM sessions WHERE expires_at <= NOW()")
                .execute(&self.pool)
                .await
                .map_err(map_error)?;
            sqlx::query("INSERT INTO sessions(token_digest,user_id) VALUES ($1,$2)")
                .bind(digest)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            tx.commit().await.map_err(map_error)
        })
    }

    fn session_user(&self, digest: &str) -> BoxFuture<'_, StorageResult<User>> {
        let digest = digest.to_owned();
        Box::pin(async move {
            let row = sqlx::query("SELECT u.id,u.email,u.display_name FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.token_digest=$1 AND s.expires_at > NOW()")
                .bind(digest).fetch_one(&self.pool).await.map_err(map_error)?;
            Ok(User {
                id: UserId::new(row.get::<Uuid, _>("id").to_string()),
                email: row.get("email"),
                display_name: row.get("display_name"),
            })
        })
    }

    fn delete_session(&self, digest: &str) -> BoxFuture<'_, StorageResult<()>> {
        let digest = digest.to_owned();
        Box::pin(async move {
            sqlx::query("DELETE FROM sessions WHERE token_digest=$1")
                .bind(digest)
                .execute(&self.pool)
                .await
                .map_err(map_error)?;
            Ok(())
        })
    }

    fn health(&self) -> BoxFuture<'_, StorageResult<()>> {
        Box::pin(async {
            sqlx::query("SELECT 1")
                .execute(&self.pool)
                .await
                .map_err(map_error)?;
            Ok(())
        })
    }

    fn get_user(&self, id: &UserId) -> BoxFuture<'_, StorageResult<User>> {
        let id = Uuid::parse_str(id.as_str());
        Box::pin(async move {
            let id = id.map_err(|_| StorageError::InvalidData("invalid user id".into()))?;
            let row = sqlx::query("SELECT id, email, display_name FROM users WHERE id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(map_error)?;
            Ok(User {
                id: UserId::new(row.get::<Uuid, _>("id").to_string()),
                email: row.get("email"),
                display_name: row.get("display_name"),
            })
        })
    }

    fn save_user(&self, user: &User) -> BoxFuture<'_, StorageResult<()>> {
        let user = user.clone();
        Box::pin(async move {
            let id = Uuid::parse_str(user.id.as_str())
                .map_err(|_| StorageError::InvalidData("invalid user id".into()))?;
            sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)")
                .bind(id)
                .bind(user.email)
                .bind(user.display_name)
                .execute(&self.pool)
                .await
                .map_err(map_error)?;
            Ok(())
        })
    }
}
