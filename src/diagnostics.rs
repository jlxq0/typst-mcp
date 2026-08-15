//! Compiler diagnostics, flattened into something a caller can act on.
//!
//! When a compile fails these *are* the result — the model reads them and fixes its
//! own source — so they carry file, line and column rather than a rendered blob.
//!
//! Byte offsets are converted with Typst's own line index, never by slicing. Slicing
//! a byte offset that lands inside a multi-byte character panics, and with
//! `panic = "abort"` in release that takes the process down. The sibling `jmap-mcp`
//! records exactly that outage.

use serde::{Deserialize, Serialize};
use typst::WorldExt;
use typst_library::World;
use typst_library::diag::{Severity as TypstSeverity, SourceDiagnostic};

/// How bad a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl From<TypstSeverity> for Severity {
    fn from(value: TypstSeverity) -> Self {
        match value {
            TypstSeverity::Error => Self::Error,
            TypstSeverity::Warning => Self::Warning,
        }
    }
}

/// One error or warning, positioned in the caller's own source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Bundle-relative path, absent when the span points at no file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line, as an editor would show it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// 1-based column, counted in characters rather than bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
}

impl Diagnostic {
    /// Convert one Typst diagnostic, resolving its span against `world`.
    pub fn from_typst(world: &dyn World, diag: &SourceDiagnostic) -> Self {
        let mut file = None;
        let mut line = None;
        let mut col = None;

        // A span can point at no file at all (diagnostics raised outside any source),
        // so every step here is fallible and simply yields less position information.
        if let Some(id) = diag.span.id() {
            file = Some(id.vpath().get_without_slash().to_owned());
            if let (Some(range), Ok(source)) = (world.range(diag.span), world.source(id)) {
                let lines = source.lines();
                line = lines.byte_to_line(range.start).map(|l| l + 1);
                col = lines.byte_to_column(range.start).map(|c| c + 1);
            }
        }

        Self {
            severity: diag.severity.into(),
            file,
            line,
            col,
            message: diag.message.to_string(),
            hints: diag.hints.iter().map(|h| h.v.to_string()).collect(),
        }
    }

    /// A diagnostic that is not tied to any source location.
    pub fn bare(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            file: None,
            line: None,
            col: None,
            message: message.into(),
            hints: vec![],
        }
    }

    /// Attach a hint. Hints are what turn an error into a fix.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }
}

/// Convert a batch, preserving order.
pub fn collect(world: &dyn World, diags: &[SourceDiagnostic]) -> Vec<Diagnostic> {
    diags
        .iter()
        .map(|d| Diagnostic::from_typst(world, d))
        .collect()
}
