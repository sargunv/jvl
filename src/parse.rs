use jsonc_parser::ParseOptions;
use jsonc_parser::ast::Value as AstValue;
use jsonc_parser::common::Ranged;
use jsonc_parser::parse_to_ast;
use jsonschema::paths::LocationSegment;
use std::ops::Range;

/// Result of parsing a JSONC file.
pub struct ParsedFile<'a> {
    /// The raw source text.
    pub source: &'a str,
    /// The parsed AST (retains byte-offset ranges for span resolution).
    pub ast: AstValue<'a>,
    /// The serde_json::Value derived from the AST (for schema validation).
    pub value: serde_json::Value,
    /// Precomputed line start offsets for fast offset-to-line/col conversion.
    line_starts: Vec<usize>,
}

impl<'a> ParsedFile<'a> {
    /// Convert a byte offset to a 1-based (line, column) pair.
    pub fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        offset_to_line_col(&self.line_starts, offset)
    }

    /// Resolve a JSON Pointer (as an iterator of LocationSegment) to a byte
    /// range in the source file by walking the AST.
    pub fn resolve_pointer<'b>(
        &self,
        segments: impl IntoIterator<Item = LocationSegment<'b>>,
    ) -> Option<Range<usize>> {
        resolve_pointer_in_ast(&self.ast, segments)
    }
}

/// Standard parse options: comments + trailing commas allowed.
pub fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_missing_commas: false,
        allow_unary_plus_numbers: false,
    }
}

/// Strip UTF-8 BOM if present.
pub fn strip_bom(source: &str) -> &str {
    source.strip_prefix('\u{FEFF}').unwrap_or(source)
}

/// Parse a JSONC source string into a `ParsedFile`.
///
/// Returns `Ok(ParsedFile)` on success, or `Err` with parse error diagnostics.
pub fn parse_jsonc(source: &str) -> Result<ParsedFile<'_>, Vec<ParseError>> {
    let source = strip_bom(source);
    let result = parse_to_ast(source, &Default::default(), &parse_options());
    match result {
        Ok(result) => match result.value {
            Some(ast) => {
                let value: serde_json::Value = ast.clone().into();
                let line_starts = compute_line_starts(source);
                Ok(ParsedFile {
                    source,
                    ast,
                    value,
                    line_starts,
                })
            }
            None => Err(vec![ParseError {
                message: "File contains no JSON value".into(),
                range: None,
            }]),
        },
        Err(err) => {
            let range = err.range();
            Err(vec![ParseError {
                message: err.to_string(),
                range: Some(range.start..range.end),
            }])
        }
    }
}

/// A parse error with an optional source range.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub range: Option<Range<usize>>,
}

/// Walk the AST following JSON Pointer segments.
fn resolve_pointer_in_ast<'a, 'b>(
    ast: &'a AstValue,
    segments: impl IntoIterator<Item = LocationSegment<'b>>,
) -> Option<Range<usize>> {
    let mut current = ast;
    for seg in segments {
        match seg {
            LocationSegment::Property(name) => {
                if let AstValue::Object(obj) = current {
                    let prop = obj
                        .properties
                        .iter()
                        .find(|p| p.name.as_str() == name.as_ref())?;
                    current = &prop.value;
                } else {
                    return None;
                }
            }
            LocationSegment::Index(idx) => {
                if let AstValue::Array(arr) = current {
                    current = arr.elements.get(idx)?;
                } else {
                    return None;
                }
            }
        }
    }
    let r = current.range();
    Some(r.start..r.end)
}

/// Precompute byte offsets where each line starts.
pub fn compute_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Convert a byte offset to a 1-based (line, column) pair using precomputed line starts.
pub fn offset_to_line_col(line_starts: &[usize], offset: usize) -> (usize, usize) {
    let line = match line_starts.binary_search(&offset) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    };
    let col = offset - line_starts[line];
    (line + 1, col + 1)
}

/// Extract the `$schema` field from a parsed JSON value.
pub fn extract_schema_field(value: &serde_json::Value) -> Option<&str> {
    value
        .as_object()
        .and_then(|obj| obj.get("$schema"))
        .and_then(|v| v.as_str())
}

/// Extract the `$schema` field from raw JSONC source without full parsing.
///
/// This is a lightweight helper for verbose diagnostics — it parses the source
/// just enough to pull the `$schema` string value. Returns an owned `String`
/// since the parsed value is temporary.
pub fn extract_schema_field_from_str(source: &str) -> Option<String> {
    let source = strip_bom(source);
    let result = parse_to_ast(source, &Default::default(), &parse_options()).ok()?;
    let ast = result.value?;
    let value: serde_json::Value = ast.into();
    extract_schema_field(&value).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jsonc_with_comments() {
        let source = r#"{
  // comment
  "key": "value",
}"#;
        let parsed = parse_jsonc(source).unwrap();
        assert_eq!(parsed.value["key"], "value");
    }

    #[test]
    fn test_offset_to_line_col() {
        let source = "abc\ndef\nghi";
        let _parsed = parse_jsonc(&format!("\"{}\"", source));
        // Manual check on line_starts
        let line_starts = compute_line_starts(source);
        assert_eq!(line_starts, vec![0, 4, 8]);
    }

    #[test]
    fn test_extract_schema_field() {
        let v: serde_json::Value =
            serde_json::json!({"$schema": "https://example.com/schema.json", "key": "val"});
        assert_eq!(
            extract_schema_field(&v),
            Some("https://example.com/schema.json")
        );
    }

    #[test]
    fn test_extract_schema_field_missing() {
        let v: serde_json::Value = serde_json::json!({"key": "val"});
        assert_eq!(extract_schema_field(&v), None);
    }
}
