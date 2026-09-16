//! Qdrant REST 适配器，所有点操作显式绑定用户与模型。
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, EmbeddingRecord, StorageError, StorageResult, VectorMatch, VectorStore,
};
use reqwest::{
    Client, Method, StatusCode, Url,
    header::{HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

pub struct QdrantStore {
    client: Client,
    base: Url,
    collection: String,
    model: String,
    dimensions: usize,
}
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid vector data or configuration".into())
}
fn unavailable() -> StorageError {
    StorageError::Unavailable("vector store operation failed".into())
}
impl QdrantStore {
    /// 创建绑定集合和模型的适配器；连接验证由 `ensure_collection` 完成。
    ///
    /// # Errors
    /// 端点、集合名、密钥或维度无效时返回错误。
    pub fn new(
        base: &str,
        collection: &str,
        key: Option<&str>,
        model: &str,
        dimensions: usize,
    ) -> StorageResult<Self> {
        let base = Url::parse(base).map_err(|_| invalid())?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
            || collection.is_empty()
            || collection.len() > 100
            || !collection
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || model.trim().is_empty()
            || !(1..=4096).contains(&dimensions)
        {
            return Err(invalid());
        }
        let mut headers = HeaderMap::new();
        if let Some(key) = key {
            if key.trim().is_empty() {
                return Err(invalid());
            }
            let mut value = HeaderValue::from_str(key).map_err(|_| invalid())?;
            value.set_sensitive(true);
            headers.insert("api-key", value);
        }
        let client = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| invalid())?;
        Ok(Self {
            client,
            base,
            collection: collection.into(),
            model: model.into(),
            dimensions,
        })
    }
    async fn request(
        &self,
        method: Method,
        suffix: &str,
        body: Option<Value>,
    ) -> StorageResult<Value> {
        let url = self
            .base
            .join(&format!("collections/{}{suffix}", self.collection))
            .map_err(|_| invalid())?;
        let mut request = self.client.request(method, url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|_| unavailable())?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(StorageError::NotFound);
        }
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err(invalid());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if value["status"] != "ok" {
            return Err(unavailable());
        }
        Ok(value["result"].clone())
    }
    /// 幂等创建集合，并拒绝已有集合的维度或距离配置不匹配。
    ///
    /// # Errors
    /// 服务不可用、创建失败或集合配置不匹配时返回错误。
    pub async fn ensure_collection(&self) -> StorageResult<()> {
        let result = match self.request(Method::GET, "", None).await {
            Ok(value) => value,
            Err(StorageError::NotFound) => {
                // 并发初始化可能由另一个实例先完成，最终以 GET 核对结果为准。
                let _ = self
                    .request(
                        Method::PUT,
                        "",
                        Some(json!({"vectors": {"size": self.dimensions, "distance": "Cosine"}})),
                    )
                    .await;
                self.request(Method::GET, "", None).await?
            }
            Err(error) => return Err(error),
        };
        let vectors = &result["config"]["params"]["vectors"];
        if vectors["size"].as_u64() != u64::try_from(self.dimensions).ok()
            || vectors["distance"] != "Cosine"
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn owner(owner: &UserId) -> StorageResult<String> {
        Uuid::parse_str(owner.as_str())
            .map(|v| v.to_string())
            .map_err(|_| invalid())
    }
    fn point_id(&self, owner: &str, id: &str) -> String {
        Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("{owner}\0{}\0{id}", self.model).as_bytes(),
        )
        .to_string()
    }
    fn filter(&self, owner: &str) -> Value {
        json!({"must": [{"key": "owner_id", "match": {"value": owner}}, {"key": "model", "match": {"value": self.model}}]})
    }
    fn vector_valid(&self, vector: &[f32]) -> bool {
        vector.len() == self.dimensions
            && vector.iter().all(|v| v.is_finite())
            && vector.iter().any(|v| *v != 0.0)
    }
}
impl VectorStore for QdrantStore {
    fn insert_embeddings(
        &self,
        owner: &UserId,
        records: &[EmbeddingRecord],
    ) -> BoxFuture<'_, StorageResult<()>> {
        let owner = Self::owner(owner);
        let records = records.to_vec();
        Box::pin(async move {
            let owner = owner?;
            if records.is_empty() || records.len() > 16 {
                return Err(invalid());
            }
            let mut points = Vec::new();
            for record in records {
                if record.model != self.model
                    || !self.vector_valid(&record.vector)
                    || record.id.is_empty()
                    || record.text.trim().is_empty()
                    || record.text.chars().count() > 1000
                    || Uuid::parse_str(&record.document_id).is_err()
                {
                    return Err(invalid());
                }
                let mut payload = serde_json::to_value(&record).map_err(|_| invalid())?;
                payload
                    .as_object_mut()
                    .ok_or_else(invalid)?
                    .remove("vector");
                points.push(json!({"id": self.point_id(&owner, &record.id), "vector": record.vector, "payload": {"owner_id": owner, "model": self.model, "record": payload}}));
            }
            let result = self
                .request(
                    Method::PUT,
                    "/points?wait=true",
                    Some(json!({"points": points})),
                )
                .await?;
            if result["status"] != "completed" {
                return Err(unavailable());
            }
            Ok(())
        })
    }
    fn similar_search(
        &self,
        owner: &UserId,
        query: &[f32],
        limit: usize,
    ) -> BoxFuture<'_, StorageResult<Vec<VectorMatch>>> {
        let owner = Self::owner(owner);
        let query = query.to_vec();
        Box::pin(async move {
            let owner = owner?;
            if !self.vector_valid(&query) || !(1..=20).contains(&limit) {
                return Err(invalid());
            }
            let result = self.request(Method::POST, "/points/search", Some(json!({"vector": query, "limit": limit, "filter": self.filter(&owner), "with_payload": true, "with_vector": true}))).await?;
            let points = result.as_array().ok_or_else(invalid)?;
            if points.len() > limit {
                return Err(invalid());
            }
            points
                .iter()
                .map(|point| {
                    let payload = &point["payload"];
                    if payload["owner_id"] != owner || payload["model"] != self.model {
                        return Err(invalid());
                    }
                    let mut record = payload["record"].clone();
                    record
                        .as_object_mut()
                        .ok_or_else(invalid)?
                        .insert("vector".into(), point["vector"].clone());
                    let record: EmbeddingRecord =
                        serde_json::from_value(record).map_err(|_| invalid())?;
                    let score: f32 =
                        serde_json::from_value(point["score"].clone()).map_err(|_| invalid())?;
                    if record.model != self.model
                        || !self.vector_valid(&record.vector)
                        || !score.is_finite()
                        || point["id"] != self.point_id(&owner, &record.id)
                    {
                        return Err(invalid());
                    }
                    Ok(VectorMatch { record, score })
                })
                .collect()
        })
    }
    fn remove(&self, owner: &UserId, ids: &[String]) -> BoxFuture<'_, StorageResult<()>> {
        let owner = Self::owner(owner);
        let ids = ids.to_vec();
        Box::pin(async move {
            let owner = owner?;
            if ids.is_empty() || ids.len() > 1000 {
                return Err(invalid());
            }
            let mut filter = self.filter(&owner);
            filter["must"].as_array_mut().ok_or_else(invalid)?.push(json!({"has_id": ids.iter().map(|id| self.point_id(&owner, id)).collect::<Vec<_>>()}));
            let result = self
                .request(
                    Method::POST,
                    "/points/delete?wait=true",
                    Some(json!({"filter": filter})),
                )
                .await?;
            if result["status"] != "completed" {
                return Err(unavailable());
            }
            Ok(())
        })
    }
}
