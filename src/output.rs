use crate::diagnostic::{FileResult, Severity, ToolDiagnostic, Warning};
use crate::schema::CacheOutcome;
use owo_colors::Stream::Stderr;
use owo_colors::{OwoColorize, Style};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

/// Summary statistics for a jvl run.
pub struct Summary {
    pub checked_files: usize,
    pub valid_files: usize,
    pub invalid_files: usize,
    pub skipped_files: usize,
    pub total_errors: usize,
    pub total_warnings: usize,
    pub duration: Duration,
    pub jobs: usize,
    pub has_tool_error: bool,
}

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Human,
    Json,
}

/// Per-file verbose diagnostic info collected during processing.
pub struct VerboseFileInfo {
    /// Display string for the resolved schema (URL or path), empty if none.
    pub schema: String,
    /// How the schema was resolved: "flag", "config", "inline $schema", or empty.
    pub schema_via: String,
    /// Disk-cache outcome for URL schemas, `None` for file schemas or in-memory hits.
    pub cache: Option<CacheOutcome>,
    /// Time spent validating this file.
    pub duration: Duration,
}

fn plural(n: usize, singular: &str, plural_form: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural_form}")
    }
}

fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        let secs = d.as_secs_f64();
        if secs < 10.0 {
            format!("{secs:.1}s")
        } else {
            format!("{}s", secs.round() as u64)
        }
    }
}

/// Write a verbose diagnostic message to stderr with dimmed styling.
pub fn verbose_log(stderr: &mut impl Write, msg: &str) {
    let line = format!("[verbose] {msg}");
    let _ = writeln!(
        stderr,
        "{}",
        line.if_supports_color(Stderr, |text| text.dimmed())
    );
}

/// Render results in human format using miette.
pub fn render_human(
    results: &[FileResult],
    warnings: &[Warning],
    summary: &Summary,
    sources: &HashMap<&str, &str>,
    stderr: &mut impl Write,
) {
    // Render warnings through miette
    for warning in warnings {
        let diag = ToolDiagnostic::warning(format!("{}: {}", warning.code, warning.message));
        let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
    }

    // Render errors per file
    for result in results {
        if result.skipped || result.errors.is_empty() {
            continue;
        }
        let source = sources.get(result.path.as_str()).copied().unwrap_or("");
        for diag in &result.errors {
            let renderable = diag.to_renderable(&result.path, source);
            let report = miette::Report::new(renderable);
            let _ = writeln!(stderr, "{report:?}");
        }
    }

    // Summary
    let _ = writeln!(stderr);
    let duration = format_duration(summary.duration);
    if summary.invalid_files == 0 {
        let msg = format!(
            "{} {} ({})",
            "✓",
            if summary.checked_files == 0 {
                "No files checked".to_string()
            } else {
                format!(
                    "All {} valid",
                    plural(summary.checked_files, "file", "files")
                )
            },
            duration,
        );
        let style = Style::new().green().bold();
        let _ = writeln!(
            stderr,
            "{}",
            msg.if_supports_color(Stderr, |text| text.style(style))
        );
        if summary.skipped_files > 0 {
            let meta = format!(
                "  Skipped {} (no schema)",
                plural(summary.skipped_files, "file", "files"),
            );
            let _ = writeln!(
                stderr,
                "{}",
                meta.if_supports_color(Stderr, |text| text.dimmed())
            );
        }
    } else {
        let primary = format!(
            "{} Found {} in {}",
            "✗",
            plural(summary.total_errors, "error", "errors"),
            plural(summary.invalid_files, "file", "files"),
        );
        let style = Style::new().red().bold();
        let _ = writeln!(
            stderr,
            "{}",
            primary.if_supports_color(Stderr, |text| text.style(style))
        );

        let mut meta = format!(
            "  Checked {}",
            plural(summary.checked_files, "file", "files"),
        );
        if summary.skipped_files > 0 {
            meta.push_str(&format!(
                ", skipped {}",
                plural(summary.skipped_files, "file", "files"),
            ));
        }
        meta.push_str(&format!(" ({duration})"));
        let _ = writeln!(
            stderr,
            "{}",
            meta.if_supports_color(Stderr, |text| text.dimmed())
        );
    }
}

// --- Typed JSON output structures ---

#[derive(Serialize)]
struct JsonOutput<'a> {
    version: u32,
    valid: bool,
    warnings: &'a [Warning],
    files: Vec<JsonFileResult>,
    summary: JsonSummary,
}

#[derive(Serialize)]
struct JsonFileResult {
    path: String,
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    errors: Vec<JsonError>,
}

#[derive(Serialize)]
struct JsonError {
    code: String,
    message: String,
    severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<JsonLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_path: Option<String>,
}

#[derive(Serialize)]
struct JsonLocation {
    line: usize,
    column: usize,
    offset: usize,
    length: usize,
}

#[derive(Serialize)]
struct JsonSummary {
    checked_files: usize,
    valid_files: usize,
    invalid_files: usize,
    skipped_files: usize,
    errors: usize,
    warnings: usize,
    duration_ms: u64,
}

/// Render results in JSON format.
///
/// When `verbose_infos` is `Some`, per-file diagnostic fields (schema, cache,
/// duration) are included in the JSON output for agent/script consumption.
pub fn render_json(
    results: &[FileResult],
    warnings: &[Warning],
    summary: &Summary,
    verbose_infos: Option<&[Option<VerboseFileInfo>]>,
    stdout: &mut impl Write,
) {
    let json_output = build_json_output(results, warnings, summary, verbose_infos);
    let json_str = serde_json::to_string_pretty(&json_output).unwrap();
    let _ = writeln!(stdout, "{json_str}");
}

fn build_json_output<'a>(
    results: &[FileResult],
    warnings: &'a [Warning],
    summary: &Summary,
    verbose_infos: Option<&[Option<VerboseFileInfo>]>,
) -> JsonOutput<'a> {
    let files: Vec<JsonFileResult> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.skipped)
        .map(|(i, r)| {
            let errors: Vec<JsonError> = r
                .errors
                .iter()
                .map(|e| {
                    let location = e.location.as_ref().map(|loc| JsonLocation {
                        line: loc.line,
                        column: loc.column,
                        offset: loc.offset,
                        length: loc.length,
                    });

                    JsonError {
                        code: e.code.clone(),
                        message: e.message.clone(),
                        severity: e.severity,
                        location,
                        schema_path: e.schema_path.clone(),
                    }
                })
                .collect();

            let (schema, schema_via, cache, duration_ms) = if let Some(infos) = verbose_infos
                && let Some(Some(info)) = infos.get(i)
            {
                let schema = if info.schema.is_empty() {
                    None
                } else {
                    Some(info.schema.clone())
                };
                let schema_via = if info.schema_via.is_empty() {
                    None
                } else {
                    Some(info.schema_via.clone())
                };
                let cache = info.cache.map(|c| c.as_str().to_string());
                let duration_ms =
                    Some(u64::try_from(info.duration.as_millis()).unwrap_or(u64::MAX));
                (schema, schema_via, cache, duration_ms)
            } else {
                (None, None, None, None)
            };

            JsonFileResult {
                path: r.path.clone(),
                valid: r.valid,
                schema,
                schema_via,
                cache,
                duration_ms,
                errors,
            }
        })
        .collect();

    JsonOutput {
        version: 1,
        valid: summary.invalid_files == 0 && !summary.has_tool_error,
        warnings,
        files,
        summary: JsonSummary {
            checked_files: summary.checked_files,
            valid_files: summary.valid_files,
            invalid_files: summary.invalid_files,
            skipped_files: summary.skipped_files,
            errors: summary.total_errors,
            warnings: summary.total_warnings,
            duration_ms: u64::try_from(summary.duration.as_millis()).unwrap_or(u64::MAX),
        },
    }
}
