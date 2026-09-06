#![forbid(unsafe_code)]

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

/// The transport adapter is intentionally deferred. This crate owns mappings
/// between internal tools and MCP protocol types, never tool business logic.
#[must_use]
pub fn empty_manifest() -> ServerManifest {
    ServerManifest {
        name: "personal-ai".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        tools: Vec::new(),
    }
}
