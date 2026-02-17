use crate::diagnostic::{FileDiagnostic, FileResult, Severity, SourceLocation, Warning};
use crate::parse::{self, ParsedFile};
use crate::schema::{SchemaCache, SchemaError, SchemaSource};
use std::path::Path;

/// Validate a single file against a resolved schema.
///
/// Returns the file result and any warnings generated during validation.
pub fn validate_file(
    file_path: &str,
    source: &str,
    schema_source: Option<&SchemaSource>,
    schema_cache: &SchemaCache,
    no_cache: bool,
    strict: bool,
) -> (FileResult, Vec<Warning>) {
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
            return (FileResult::invalid(file_path, errors), warnings);
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
            );
        }
        return (FileResult::skipped(file_path), warnings);
    };

    // Load schema and get/compile the validator
    let (validator, schema_warnings) =
        match schema_cache.get_or_compile(&effective_schema, no_cache) {
            Ok(result) => result,
            Err(e) => {
                let category = match &e {
                    SchemaError::CompileError(_) => "compile",
                    _ => "load",
                };
                return (
                    FileResult::tool_error(
                        file_path,
                        vec![FileDiagnostic {
                            code: format!("schema({category})"),
                            message: e.to_string(),
                            severity: Severity::Error,
                            span: None,
                            location: None,
                            label: None,
                            help: None,
                            schema_path: None,
                        }],
                    ),
                    warnings,
                );
            }
        };
    warnings.extend(schema_warnings);

    // Validate
    let validation_errors: Vec<_> = validator.iter_errors(&parsed.value).collect();

    if validation_errors.is_empty() {
        return (FileResult::valid(file_path), warnings);
    }

    let errors = map_validation_errors(&parsed, &validation_errors);
    (FileResult::invalid(file_path, errors), warnings)
}

/// Map jsonschema validation errors to our diagnostic format.
fn map_validation_errors(
    parsed: &ParsedFile,
    errors: &[jsonschema::ValidationError],
) -> Vec<FileDiagnostic> {
    errors
        .iter()
        .map(|err| {
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
            let code = format!("schema({keyword})");

            let message = err.to_string();
            let label = format_validation_label(err);
            let help = format_validation_help(err);

            let schema_path = err.schema_path().as_str().to_string();

            FileDiagnostic {
                code,
                message,
                severity: Severity::Error,
                span,
                location,
                label: Some(label),
                help,
                schema_path: Some(schema_path),
            }
        })
        .collect()
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
