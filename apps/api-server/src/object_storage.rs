use personal_ai_storage::{ObjectStorage, StorageError, StorageResult};
use personal_ai_storage_s3::S3Store;
use std::sync::Arc;

/// 仅在显式启用时使用 S3，避免改变已有 `PostgreSQL` 部署。
///
/// # Errors
/// 已启用但配置缺失或无效时启动失败，不回退到其他存储。
pub fn object_storage_from_env() -> StorageResult<Option<Arc<dyn ObjectStorage>>> {
    object_storage_config(|key| std::env::var(key).ok())
}

fn object_storage_config(
    env: impl Fn(&str) -> Option<String>,
) -> StorageResult<Option<Arc<dyn ObjectStorage>>> {
    match env("OBJECT_STORE_ENABLED").as_deref() {
        None | Some("false") => return Ok(None),
        Some("true") => {}
        _ => {
            return Err(StorageError::InvalidData(
                "OBJECT_STORE_ENABLED must be true or false".into(),
            ));
        }
    }
    let required = |key: &str| {
        env(key)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| StorageError::InvalidData(format!("{key} is required")))
    };
    Ok(Some(Arc::new(S3Store::new(
        &required("OBJECT_STORE_ENDPOINT")?,
        &required("OBJECT_STORE_BUCKET")?,
        &env("OBJECT_STORE_REGION").unwrap_or_else(|| "us-east-1".into()),
        &required("OBJECT_STORE_ACCESS_KEY")?,
        &required("OBJECT_STORE_SECRET_KEY")?,
    )?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_is_explicit_and_partial_configuration_fails() {
        assert!(object_storage_config(|_| None).unwrap().is_none());
        assert!(
            object_storage_config(|key| (key == "OBJECT_STORE_ENABLED").then(|| "false".into()))
                .unwrap()
                .is_none()
        );
        assert!(
            object_storage_config(|key| (key == "OBJECT_STORE_ENABLED").then(|| "true".into()))
                .is_err()
        );
        assert!(object_storage_config(|_| Some("yes".into())).is_err());
    }
}
