//! Fixed aggregate metadata for the synthetic CLI, never raw model selections or text.
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub(super) struct SupportCounts {
    pub stated: usize,
    pub unavailable: usize,
    pub unsupported: usize,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct KindCounts {
    pub value: SupportCounts,
    pub availability: SupportCounts,
    pub fact: SupportCounts,
}

#[derive(Debug, Serialize)]
pub struct SemanticDiagnostics {
    pub(super) schema: &'static str,
    pub(super) selected_excerpts: usize,
    pub(super) verified_excerpts: usize,
    pub(super) requirements: usize,
    pub(super) evidence_agrees: bool,
    pub(super) requirements_supported: bool,
    pub(super) checks_by_kind: KindCounts,
}
