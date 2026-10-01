//! 宿主专属只读检索凭据；明文仅在创建响应中出现。
#[derive(Clone, Debug, serde::Serialize)]
pub struct McpCredential {
    pub id: String,
    pub host_name: String,
    pub scope: String,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Clone)]
pub struct NewMcpCredential {
    pub digest: String,
    pub session_digest: String,
    pub host_name: String,
    pub days: i32,
}
impl NewMcpCredential {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.digest.len() == 64
            && self
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && (1..=30).contains(&self.days)
            && !self.host_name.trim().is_empty()
            && self.host_name.chars().count() <= 80
            && !self.host_name.chars().any(char::is_control)
    }
}
