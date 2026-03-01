use std::borrow::Cow;
use std::path::Path;
use std::time::{Duration, Instant};

use jsonschema::paths::LocationSegment;

use crate::diagnostic::{FileDiagnostic, FileResult, Severity, SourceLocation, Warning};
use crate::parse::{self, ParsedFile};
use crate::schema::{CacheOutcome, SchemaCache, SchemaError, SchemaSource};

/// Timing breakdown for schema compilation and validation.
#[derive(Debug, Clone, Copy)]
pub struct TimingBreakdown {
    /// Time spent loading + compiling the schema (including $ref resolution).
    pub compile: Duration,
    /// Time spent validating the document against the compiled schema.
    pub validate: Duration,
}

/// Validate a single file against a resolved schema.
///
/// Returns `(file_result, warnings, cache_outcome, timing)`. The `cache_outcome`
/// is `None` for file-based schemas, skipped files, or when the compiled schema
/// was already cached in memory by another thread. `timing` is `None` when the
/// file is skipped or has parse errors.
pub fn validate_file(
    file_path: &str,
    source: &str,
    schema_source: Option<&SchemaSource>,
    schema_cache: &SchemaCache,
    no_cache: bool,
    strict: bool,
) -> (
    FileResult,
    Vec<Warning>,
    Option<CacheOutcome>,
    Option<TimingBreakdown>,
) {
    let mut warnings = Vec::new();

    // Parse the file
    let parsed = match parse::parse_jsonc(source) {
        Ok(p) => p,
        Err(parse_errors) => {
            let line_starts = parse::compute_line_starts(source);
            let errors: Vec<FileDiagnostic> = parse_errors
                .into_iter()
                .map(|e| {
                    let location = e.range.as_ref().map(|r| {
                        let (line, col) = parse::offset_to_line_col(&line_starts, r.start);
                        SourceLocation {
                            line,
                            column: col,
                            offset: r.start,
                            length: r.len(),
                        }
                    });
                    FileDiagnostic {
                        code: "parse(syntax)".into(),
                        message: e.message,
                        severity: Severity::Error,
                        span: e.range,
                        location,
                        label: Some("syntax error".into()),
                        help: None,
                        schema_path: None,
                    }
                })
                .collect();
            return (FileResult::invalid(file_path, errors), warnings, None, None);
        }
    };

    // Determine schema source: explicit override > $schema field in file > config mapping
    let effective_schema = schema_source.cloned().or_else(|| {
        parse::extract_schema_field(&parsed.value).map(|schema_ref| {
            let base_dir = Path::new(file_path)
                .parent()
                .unwrap_or_else(|| Path::new("."));
            crate::schema::resolve_schema_ref(schema_ref, base_dir)
        })
    });

    let Some(effective_schema) = effective_schema else {
        if strict {
            return (
                FileResult::invalid(
                    file_path,
                    vec![FileDiagnostic {
                        code: "no-schema".into(),
                        message: "no schema found".into(),
                        severity: Severity::Error,
                        span: None,
                        location: None,
                        label: None,
                        help: Some(
                            "Add a \"$schema\" field to the file, configure a schema mapping \
                             in jvl.json, or use --schema."
                                .into(),
                        ),
                        schema_path: None,
                    }],
                ),
                warnings,
                None,
                None,
            );
        }
        return (FileResult::skipped(file_path), warnings, None, None);
    };

    // Load schema and get/compile the validator
    let compile_start = Instant::now();
    let (validator, schema_warnings, cache_outcome) =
        match schema_cache.get_or_compile(&effective_schema, no_cache) {
            Ok(result) => result,
            Err(e) => {
                let category = match &e {
                    SchemaError::CompileError(_) => "compile",
                    _ => "load",
                };
                // Point at the $schema value span when the schema came from the
                // document's $schema field (not from --schema flag or jvl.json mapping).
                let span = if schema_source.is_none() {
                    parsed.resolve_pointer(std::iter::once(LocationSegment::Property(
                        Cow::Borrowed("$schema"),
                    )))
                } else {
                    None
                };
                let location = span.as_ref().map(|r| {
                    let (line, col) = parsed.offset_to_line_col(r.start);
                    SourceLocation {
                        line,
                        column: col,
                        offset: r.start,
                        length: r.len(),
                    }
                });
                let label = span.as_ref().map(|_| "schema referenced here".into());
                // When the span points at the $schema value, the path is
                // already visible in the source snippet — just show the reason.
                let message = if span.is_some() {
                    e.reason().to_string()
                } else {
                    e.to_string()
                };
                return (
                    FileResult::tool_error(
                        file_path,
                        vec![FileDiagnostic {
                            code: format!("schema({category})"),
                            message,
                            severity: Severity::Error,
                            span,
                            location,
                            label,
                            help: None,
                            schema_path: None,
                        }],
                    ),
                    warnings,
                    None,
                    None,
                );
            }
        };
    let compile_duration = compile_start.elapsed();
    warnings.extend(schema_warnings);

    // Validate
    let validate_start = Instant::now();
    let validation_errors: Vec<_> = validator.iter_errors(&parsed.value).collect();
    let validate_duration = validate_start.elapsed();

    let timing = Some(TimingBreakdown {
        compile: compile_duration,
        validate: validate_duration,
    });

    if validation_errors.is_empty() {
        return (
            FileResult::valid(file_path),
            warnings,
            cache_outcome,
            timing,
        );
    }

    let errors = map_validation_errors(&parsed, &validation_errors);
    (
        FileResult::invalid(file_path, errors),
        warnings,
        cache_outcome,
        timing,
    )
}

/// Map jsonschema validation errors to our diagnostic format.
///
/// Most errors produce one `FileDiagnostic`. A few produce one per offending
/// item so each gets its own editor squiggle:
/// - `additionalProperties` / `unevaluatedProperties`: one per extra property
/// - `additionalItems`: one per extra array element
fn map_validation_errors(
    parsed: &ParsedFile,
    errors: &[jsonschema::ValidationError],
) -> Vec<FileDiagnostic> {
    use jsonschema::error::ValidationErrorKind;
    use jsonschema::paths::LocationSegment;

    let mut result = Vec::new();

    for err in errors {
        let schema_path = err.schema_path().as_str().to_string();
        let base: Vec<LocationSegment<'_>> = err.instance_path().iter().collect();
        // Re-borrow base segments so they can be chained with a new tail segment.
        let base_iter = || {
            base.iter().map(|s| match s {
                LocationSegment::Property(p) => {
                    LocationSegment::Property(Cow::Borrowed(p.as_ref()))
                }
                LocationSegment::Index(i) => LocationSegment::Index(*i),
            })
        };

        match err.kind() {
            // Emit one diagnostic per unexpected property so each gets its own squiggle.
            // The squiggle lands on the property key, not the value.
            ValidationErrorKind::AdditionalProperties { unexpected }
            | ValidationErrorKind::UnevaluatedProperties { unexpected } => {
                let keyword = err.kind().keyword();
                let is_unevaluated = matches!(
                    err.kind(),
                    ValidationErrorKind::UnevaluatedProperties { .. }
                );
                let term = if is_unevaluated {
                    "unevaluated"
                } else {
                    "additional"
                };
                for prop_name in unexpected {
                    let span = parsed.resolve_pointer_key(base_iter().chain(std::iter::once(
                        LocationSegment::Property(Cow::Borrowed(prop_name.as_str())),
                    )));
                    let location = span.as_ref().map(|r| {
                        let (line, col) = parsed.offset_to_line_col(r.start);
                        SourceLocation {
                            line,
                            column: col,
                            offset: r.start,
                            length: r.len(),
                        }
                    });
                    result.push(FileDiagnostic {
                        code: format!("schema({keyword})"),
                        message: format!("{term} property '{prop_name}' is not allowed"),
                        severity: Severity::Error,
                        span,
                        location,
                        label: Some("unexpected property".into()),
                        help: Some(
                            "Remove the property, or check for typos in the property name.".into(),
                        ),
                        schema_path: Some(schema_path.clone()),
                    });
                }
            }

            // Emit one diagnostic per extra array item.
            ValidationErrorKind::AdditionalItems { limit } => {
                let mut any_emitted = false;
                let mut idx = *limit;
                loop {
                    let span = parsed.resolve_pointer(
                        base_iter().chain(std::iter::once(LocationSegment::Index(idx))),
                    );
                    let Some(span) = span else { break };
                    let location = {
                        let (line, col) = parsed.offset_to_line_col(span.start);
                        Some(SourceLocation {
                            line,
                            column: col,
                            offset: span.start,
                            length: span.len(),
                        })
                    };
                    result.push(FileDiagnostic {
                        code: "schema(additionalItems)".into(),
                        message: format!("item at index {idx} is not allowed"),
                        severity: Severity::Error,
                        span: Some(span),
                        location,
                        label: Some("extra item not allowed".into()),
                        help: Some(
                            "Remove the extra items, or update the schema to allow more.".into(),
                        ),
                        schema_path: Some(schema_path.clone()),
                    });
                    idx += 1;
                    any_emitted = true;
                }
                // Fallback to whole-array span if no items were resolved.
                if !any_emitted {
                    result.push(make_diagnostic(parsed, err, &schema_path));
                }
            }

            _ => result.push(make_diagnostic(parsed, err, &schema_path)),
        }
    }

    result
}

/// Build a single `FileDiagnostic` from a validation error using the standard
/// instance_path → span resolution.
fn make_diagnostic(
    parsed: &ParsedFile,
    err: &jsonschema::ValidationError,
    schema_path: &str,
) -> FileDiagnostic {
    let instance_path = err.instance_path();
    let span = parsed.resolve_pointer(instance_path.iter());
    let location = span.as_ref().map(|r| {
        let (line, col) = parsed.offset_to_line_col(r.start);
        SourceLocation {
            line,
            column: col,
            offset: r.start,
            length: r.len(),
        }
    });
    let keyword = err.kind().keyword();
    FileDiagnostic {
        code: format!("schema({keyword})"),
        message: err.to_string(),
        severity: Severity::Error,
        span,
        location,
        label: Some(format_validation_label(err)),
        help: format_validation_help(err),
        schema_path: Some(schema_path.to_string()),
    }
}

/// Format a short label for the source span.
fn format_validation_label(err: &jsonschema::ValidationError) -> String {
    use jsonschema::error::{TypeKind, ValidationErrorKind};
    match err.kind() {
        ValidationErrorKind::Type {
            kind: TypeKind::Single(t),
        } => format!("expected type \"{t}\""),
        ValidationErrorKind::Type {
            kind: TypeKind::Multiple(types),
        } => {
            let type_strs: Vec<_> = types.iter().map(|t| format!("\"{t}\"")).collect();
            format!("expected one of types {}", type_strs.join(", "))
        }
        ValidationErrorKind::Required { .. } => "required property missing here".into(),
        ValidationErrorKind::Enum { .. } => "value not in allowed set".into(),
        ValidationErrorKind::Constant { .. } => "value doesn't match expected constant".into(),
        ValidationErrorKind::Pattern { .. }
        | ValidationErrorKind::BacktrackLimitExceeded { .. } => {
            "value doesn't match pattern".into()
        }
        ValidationErrorKind::Minimum { .. }
        | ValidationErrorKind::Maximum { .. }
        | ValidationErrorKind::ExclusiveMinimum { .. }
        | ValidationErrorKind::ExclusiveMaximum { .. } => "value out of range".into(),
        ValidationErrorKind::MinLength { .. } | ValidationErrorKind::MaxLength { .. } => {
            "string length out of range".into()
        }
        ValidationErrorKind::MinItems { .. } | ValidationErrorKind::MaxItems { .. } => {
            "array length out of range".into()
        }
        ValidationErrorKind::MinProperties { .. } | ValidationErrorKind::MaxProperties { .. } => {
            "property count out of range".into()
        }
        ValidationErrorKind::MultipleOf { .. } => "value is not a valid multiple".into(),
        ValidationErrorKind::UniqueItems => "array has duplicate items".into(),
        // These two kinds are fully handled by the special-case branches in
        // map_validation_errors and never reach make_diagnostic. The arms are
        // kept here only to satisfy the exhaustive match.
        ValidationErrorKind::AdditionalProperties { .. }
        | ValidationErrorKind::UnevaluatedProperties { .. } => "unexpected property".into(),
        ValidationErrorKind::AdditionalItems { .. } => "extra item not allowed".into(),
        ValidationErrorKind::UnevaluatedItems { .. } => "unexpected item".into(),
        ValidationErrorKind::AnyOf { .. } | ValidationErrorKind::OneOfNotValid { .. } => {
            "no matching schema".into()
        }
        ValidationErrorKind::OneOfMultipleValid { .. } => "multiple schemas matched".into(),
        ValidationErrorKind::Not { .. } => "value is disallowed".into(),
        ValidationErrorKind::FalseSchema => "no value allowed here".into(),
        ValidationErrorKind::Format { .. } => "value doesn't match expected format".into(),
        ValidationErrorKind::Contains => "no matching item found".into(),
        ValidationErrorKind::PropertyNames { .. } => "invalid property name".into(),
        ValidationErrorKind::ContentEncoding { .. } | ValidationErrorKind::FromUtf8 { .. } => {
            "invalid content encoding".into()
        }
        ValidationErrorKind::ContentMediaType { .. } => "invalid content media type".into(),
        ValidationErrorKind::Referencing(_) => "schema reference error".into(),
        ValidationErrorKind::Custom { .. } => {
            format!("{} validation failed", err.kind().keyword())
        }
    }
}

/// Format help text for a validation error. Returns `None` when the error
/// message is already self-explanatory.
fn format_validation_help(err: &jsonschema::ValidationError) -> Option<String> {
    use jsonschema::error::ValidationErrorKind;
    match err.kind() {
        ValidationErrorKind::Required { .. } => {
            Some("Add the missing property to this object.".into())
        }
        // Kept for exhaustive match; see comment in format_validation_label.
        ValidationErrorKind::AdditionalProperties { .. }
        | ValidationErrorKind::UnevaluatedProperties { .. } => {
            Some("Remove the property, or check for typos in the property name.".into())
        }
        ValidationErrorKind::AdditionalItems { .. }
        | ValidationErrorKind::UnevaluatedItems { .. } => {
            Some("Remove the extra items, or update the schema to allow more.".into())
        }
        ValidationErrorKind::AnyOf { .. } => {
            Some("The value must match at least one of the listed schemas.".into())
        }
        ValidationErrorKind::OneOfNotValid { .. } => {
            Some("The value must match exactly one of the listed schemas.".into())
        }
        ValidationErrorKind::OneOfMultipleValid { .. } => {
            Some("The value must match exactly one schema, but it matches multiple.".into())
        }
        ValidationErrorKind::Not { .. } => {
            Some("The value is explicitly disallowed by a 'not' constraint in the schema.".into())
        }
        ValidationErrorKind::FalseSchema => Some("This location does not allow any value.".into()),
        ValidationErrorKind::PropertyNames { .. } => {
            Some("One or more property names are invalid.".into())
        }
        ValidationErrorKind::Referencing(_) => Some(
            "The schema could not resolve a reference. The schema itself may be broken.".into(),
        ),
        _ => None,
    }
}
