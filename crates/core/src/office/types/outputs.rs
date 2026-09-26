use super::operations::{OfficeDocumentKind, OfficeOperation, OfficePathScope};
use crate::AgentFileInputRef;
use serde::{Deserialize, Serialize};

/// Bounded provider outcome for the Host-owned presentation edit transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficePresentationEditResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub error_code: Option<String>,
    pub error: Option<String>,
}

/// Stable diagnostics returned by the format-specific validation and atomic publication gate for
/// a provenance-bound Office Skill script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficeManagedScriptOutputResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub error_code: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeExecutionResult {
    pub provider_id: String,
    pub engine_revision: String,
    pub document_kind: OfficeDocumentKind,
    pub operation: OfficeOperation,
    /// Authoritative files published by this execution.
    ///
    /// This model-actionable field intentionally precedes low-level process
    /// diagnostics in the serialized result. Entries are added only after
    /// validation and atomic publication succeed. `read_path` always
    /// identifies the final target and never a staging or private snapshot
    /// path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OfficePublishedOutput>,
    /// Frozen logical argv used for diagnostics and approval review. It is the
    /// exact provider argv for ordinary OfficeCLI operations; Host-managed
    /// adapters such as Word PDF conversion retain canonical logical tokens so
    /// private runtime paths are never exposed. This is not a command string
    /// and cannot be replayed through a shell.
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(flatten, default)]
    pub output_capture: crate::command::ProcessOutputCaptureMetadata,
    /// Backend-only complete stdout capture consumed by Exact History.
    #[serde(skip, default)]
    pub stdout_spool: crate::command::ProcessOutputSpool,
    /// Backend-only complete stderr capture consumed by Exact History.
    #[serde(skip, default)]
    pub stderr_spool: crate::command::ProcessOutputSpool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl OfficeExecutionResult {
    pub fn output_spool_substitutions(
        &self,
    ) -> Vec<crate::command::ProcessOutputSpoolSubstitution> {
        crate::command::process_output_spool_substitutions(&self.stdout_spool, &self.stderr_spool)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficePublishedOutputRole {
    Render,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficePublishedOutputKind {
    Image,
    Document,
}

/// The page/slide selection requested for a rendered output.
///
/// This records selection intent, not independently verified per-page
/// coverage. Provider success and artifact validation prove that the published
/// file is valid, but do not prove that every requested page appears in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OfficeRenderPageSelection {
    All,
    Explicit { pages: Vec<u32> },
}

/// Host-verified layout geometry for one presentation screenshot.
///
/// This receipt has deliberately narrow semantics: it proves that the frozen
/// requested slide set fits inside the decoded PNG viewport according to the
/// trusted renderer's fixed layout formula. It does not prove that each slide's
/// visual content rendered correctly and cannot replace per-slide `read_image`
/// inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeRenderLayoutCoverage {
    /// Exact, canonical page/slide set used by the frozen layout plan.
    pub requested_pages: Vec<u32>,
    /// The sole evidence class accepted by this receipt.
    pub evidence: OfficeRenderLayoutEvidence,
    /// Verified contact-sheet geometry. Single-slide renders omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<OfficeRenderGridGeometry>,
}

/// Evidence used for an [`OfficeRenderLayoutCoverage`] receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeRenderLayoutEvidence {
    TrustedRendererGeometry,
}

/// Integer geometry from the Host-owned contact-sheet plan after it has been
/// checked against the decoded image dimensions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeRenderGridGeometry {
    pub columns: u16,
    pub rows: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub content_width: u32,
    pub content_height: u32,
}

/// One validated Office artifact at its final, atomically published location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePublishedOutput {
    pub role: OfficePublishedOutputRole,
    pub kind: OfficePublishedOutputKind,
    pub mime_type: String,
    /// Typed reference that can be reused by tools accepting
    /// [`AgentFileInputRef`]. It remains subject to the current read policy.
    pub source: AgentFileInputRef,
    /// Logical final path that can be supplied to ordinary file-reading tools.
    /// Every consumer still enforces its own format, size, and delivery limits.
    pub read_path: String,
    pub scope: OfficePathScope,
    /// Whether the current Agent read permission covers `read_path`.
    ///
    /// This is not a promise that a downstream visual/file tool accepts the
    /// artifact's MIME type, dimensions, or byte size.
    pub readable_by_agent: bool,
    pub size_bytes: u64,
    /// Lowercase hexadecimal SHA-256 of the published bytes.
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Authoritative physical PDF page count verified by the Host. Present
    /// only for the managed Word-to-PDF render path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    /// SHA-256 of the frozen source DOCX bytes used for this render. This lets
    /// consumers invalidate QA evidence after any source revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    /// Immutable revision of the Host-managed renderer which produced this
    /// artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renderer_revision: Option<String>,
    pub page_selection: OfficeRenderPageSelection,
    /// Host-verified renderer layout geometry. This is not visual-quality or
    /// per-slide content evidence; final slides still require individual visual
    /// inspection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_coverage: Option<OfficeRenderLayoutCoverage>,
}
