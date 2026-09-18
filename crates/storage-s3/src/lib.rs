//! `MinIO` / S3 原文适配器；SDK 类型与凭据不进入领域接口。
use futures::TryStreamExt;
use object_store::{
    Attribute, Attributes, ObjectStore, PutOptions, aws::AmazonS3Builder, path::Path,
};
use personal_ai_storage::{BoxFuture, ObjectInfo, ObjectStorage, StorageError, StorageResult};
use std::time::Duration;

pub struct S3Store {
    inner: object_store::aws::AmazonS3,
}

impl S3Store {
    /// 使用显式配置连接已有的私有桶，不读取隐式云凭据。
    ///
    /// # Errors
    /// 配置无效时返回不含凭据的错误。
    pub fn new(
        endpoint: &str,
        bucket: &str,
        region: &str,
        access_key: &str,
        secret_key: &str,
    ) -> StorageResult<Self> {
        let invalid = || StorageError::InvalidData("invalid object store configuration".into());
        let url = url::Url::parse(endpoint).map_err(|_| invalid())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || [bucket, region, access_key, secret_key]
                .iter()
                .any(|s| s.trim().is_empty())
        {
            return Err(invalid());
        }
        let inner = AmazonS3Builder::new()
            .with_endpoint(endpoint)
            .with_bucket_name(bucket)
            .with_region(region)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key)
            .with_virtual_hosted_style_request(false)
            .with_client_options(
                object_store::ClientOptions::new()
                    .with_allow_http(url.scheme() == "http")
                    .with_timeout(Duration::from_secs(15)),
            )
            .with_retry(object_store::RetryConfig {
                max_retries: 2,
                retry_timeout: Duration::from_secs(30),
                ..Default::default()
            })
            .build()
            .map_err(|_| invalid())?;
        Ok(Self { inner })
    }
}

fn path(key: &str) -> StorageResult<Path> {
    if key.is_empty()
        || key.starts_with('/')
        || key.ends_with('/')
        || key
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(StorageError::InvalidData("invalid object key".into()));
    }
    Path::parse(key).map_err(|_| StorageError::InvalidData("invalid object key".into()))
}

#[allow(clippy::needless_pass_by_value)] // 直接用于 Result::map_err。
fn map_error(error: object_store::Error) -> StorageError {
    match error {
        object_store::Error::NotFound { .. } => StorageError::NotFound,
        _ => StorageError::Unavailable("object store operation failed".into()),
    }
}

impl ObjectStorage for S3Store {
    fn list_originals(&self, after: &str) -> BoxFuture<'_, StorageResult<Vec<ObjectInfo>>> {
        let after = after.to_owned();
        Box::pin(async move {
            let prefix = Path::from("users");
            let offset = if after.is_empty() {
                Path::from("users/")
            } else {
                path(&after)?
            };
            let mut stream = self.inner.list_with_offset(Some(&prefix), &offset);
            let mut result = Vec::new();
            while result.len() < 100 {
                let Some(meta) = stream.try_next().await.map_err(map_error)? else {
                    break;
                };
                result.push(ObjectInfo {
                    key: meta.location.to_string(),
                    // HEAD 的 HTTP 时间只有秒精度，统一精度才能与列举结果核对。
                    modified_unix_ms: meta.last_modified.timestamp() * 1000,
                });
            }
            Ok(result)
        })
    }
    fn head(&self, key: &str) -> BoxFuture<'_, StorageResult<ObjectInfo>> {
        let key = path(key);
        Box::pin(async move {
            let meta = self.inner.head(&key?).await.map_err(map_error)?;
            Ok(ObjectInfo {
                key: meta.location.to_string(),
                modified_unix_ms: meta.last_modified.timestamp() * 1000,
            })
        })
    }
    fn put(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let path = path(key);
        let bytes = bytes.to_vec();
        let mut attributes = Attributes::new();
        if let Some(content_type) = content_type {
            attributes.insert(Attribute::ContentType, content_type.to_owned().into());
        }
        Box::pin(async move {
            self.inner
                .put_opts(
                    &path?,
                    bytes.into(),
                    PutOptions {
                        attributes,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| StorageError::Unavailable("object store write failed".into()))?;
            Ok(())
        })
    }

    fn get(&self, key: &str) -> BoxFuture<'_, StorageResult<Vec<u8>>> {
        let path = path(key);
        Box::pin(async move {
            let result = self.inner.get(&path?).await.map_err(map_error)?;
            Ok(result.bytes().await.map_err(map_error)?.to_vec())
        })
    }

    fn delete(&self, key: &str) -> BoxFuture<'_, StorageResult<()>> {
        let path = path(key);
        Box::pin(async move { self.inner.delete(&path?).await.map_err(map_error) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_keys_and_credentials_in_endpoints() {
        for key in ["", "/a", "a/", "a//b", "a/../b", "a/./b"] {
            assert!(path(key).is_err(), "{key}");
        }
        assert!(path("users/u/documents/d/original.pdf").is_ok());
        for endpoint in [
            "ftp://localhost",
            "http://user:secret@localhost",
            "http://localhost/path",
            "http://localhost?token=secret",
        ] {
            assert!(S3Store::new(endpoint, "bucket", "us-east-1", "key", "secret").is_err());
        }
    }
}
