use miette::{Diagnostic, SourceSpan};
use std::ops::Range;
use thiserror::Error;

/// The severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Resolved source location for a diagnostic.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
    pub offset: usize,
    pub length: usize,
}

/// A structured diagnostic produced by jvl.
#[derive(Debug, Clone)]
pub struct FileDiagnostic {
    pub code: String,
    pub message: String,
    pub severity: Severity,
    pub span: Option<Range<usize>>,
    pub location: Option<SourceLocation>,
    pub label: Option<String>,
    pub help: Option<String>,
    pub schema_path: Option<String>,
}

/// A warning not tied to a specific file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// The result of checking a single file.
#[derive(Debug, Clone)]
pub struct FileResult {
    pub path: String,
    pub valid: bool,
    pub errors: Vec<FileDiagnostic>,
    pub skipped: bool,
    /// Whether this result represents a tool error (exit code 2) rather than
    /// a validation error (exit code 1).
    pub tool_error: bool,
}

impl FileResult {
    pub fn valid(path: &str) -> Self {
        Self {
            path: path.to_string(),
            valid: true,
            errors: vec![],
            skipped: false,
            tool_error: false,
        }
    }

    pub fn skipped(path: &str) -> Self {
        Self {
            path: path.to_string(),
            valid: true,
            errors: vec![],
            skipped: true,
            tool_error: false,
        }
    }

    pub fn invalid(path: &str, errors: Vec<FileDiagnostic>) -> Self {
        Self {
            path: path.to_string(),
            valid: false,
            errors,
            skipped: false,
            tool_error: false,
        }
    }

    pub fn tool_error(path: &str, errors: Vec<FileDiagnostic>) -> Self {
        Self {
            path: path.to_string(),
            valid: false,
            errors,
            skipped: false,
            tool_error: true,
        }
    }
}

/// Miette-compatible error for rendering rich diagnostics.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct RenderableDiagnostic {
    pub message: String,
    pub src: miette::NamedSource<String>,
    pub span: Option<SourceSpan>,
    pub label: Option<String>,
    pub help: Option<String>,
}

impl Diagnostic for RenderableDiagnostic {
    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.src)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        self.span.map(|span| {
            Box::new(std::iter::once(miette::LabeledSpan::new(
                self.label.clone(),
                span.offset(),
                span.len(),
            ))) as Box<dyn Iterator<Item = miette::LabeledSpan>>
        })
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h.as_str()) as Box<dyn std::fmt::Display>)
    }
}

/// A lightweight diagnostic for tool-level errors/warnings that don't have source code.
///
/// Renders through miette as:
///   × failed to load config: parse error at line 3
///   ⚠ --jobs clamped to 1 (was 0)
#[derive(Debug, Error)]
#[error("{message}")]
pub struct ToolDiagnostic {
    message: String,
    severity: miette::Severity,
    help_text: Option<String>,
}

impl ToolDiagnostic {
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            severity: miette::Severity::Error,
            help_text: None,
        }
    }

    pub fn warning(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            severity: miette::Severity::Warning,
            help_text: None,
        }
    }
}

impl Diagnostic for ToolDiagnostic {
    fn severity(&self) -> Option<miette::Severity> {
        Some(self.severity)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help_text
            .as_ref()
            .map(|h| Box::new(h.as_str()) as Box<dyn std::fmt::Display>)
    }
}

impl FileDiagnostic {
    /// Convert to a miette-renderable diagnostic.
    pub fn to_renderable(&self, file_path: &str, source: &str) -> RenderableDiagnostic {
        // Fall back to a zero-length span at offset 0 so miette always renders
        // the "╭─[filename:1:1]" header, even for errors without a source location.
        let span = Some(match &self.span {
            Some(r) => SourceSpan::new(r.start.into(), r.len()),
            None => SourceSpan::new(0.into(), 0),
        });
        RenderableDiagnostic {
            message: format!("{}: {}", self.code, self.message),
            src: miette::NamedSource::new(file_path, source.to_owned()),
            span,
            label: self.label.clone(),
            help: self.help.clone(),
        }
    }
}
