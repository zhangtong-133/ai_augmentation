#![forbid(unsafe_code)]
pub mod bridge;
pub mod protocol;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExposedTool {
    pub name: String,
    pub description: String,
    pub input_schema_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerManifest {
    pub name: String,
    pub version: String,
    pub tools: Vec<ExposedTool>,
}

/// 本 crate 提供本地 stdio 桥接与内部工具的 MCP 协议类型
/// 之间的映射，不包含工具的业务逻辑。
#[must_use]
pub fn empty_manifest() -> ServerManifest {
    ServerManifest {
        name: "personal-ai".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        tools: Vec::new(),
    }
}
