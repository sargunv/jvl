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

    /// Like [`resolve_pointer`], but returns the span of the **key token**
    /// for the final `Property` segment instead of its value.
    ///
    /// Returns `None` if any segment cannot be resolved, if there are no
    /// segments, or if the final segment is an array index.
    pub fn resolve_pointer_key<'b>(
        &self,
        segments: impl IntoIterator<Item = LocationSegment<'b>>,
    ) -> Option<Range<usize>> {
        resolve_pointer_key_in_ast(&self.ast, segments)
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

/// Walk the AST following JSON Pointer segments, returning the key span of
/// the final `Property` segment rather than its value.
///
/// Returns `None` for an empty segment sequence, if any segment is
/// unresolvable, or if the final segment is an array index.
fn resolve_pointer_key_in_ast<'a, 'b>(
    ast: &'a AstValue,
    segments: impl IntoIterator<Item = LocationSegment<'b>>,
) -> Option<Range<usize>> {
    let mut current = ast;
    let mut segs = segments.into_iter().peekable();
    while let Some(seg) = segs.next() {
        let is_last = segs.peek().is_none();
        match seg {
            LocationSegment::Property(name) => {
                if let AstValue::Object(obj) = current {
                    let prop = obj
                        .properties
                        .iter()
                        .find(|p| p.name.as_str() == name.as_ref())?;
                    if is_last {
                        let r = prop.name.range();
                        return Some(r.start..r.end);
                    }
                    current = &prop.value;
                } else {
                    return None;
                }
            }
            LocationSegment::Index(idx) => {
                if let AstValue::Array(arr) = current {
                    let elem = arr.elements.get(idx)?;
                    if is_last {
                        return None; // array elements have no key token
                    }
                    current = elem;
                } else {
                    return None;
                }
            }
        }
    }
    None // empty segment sequence — no key to return
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

/// Find the JSON pointer path for the node at a given byte offset.
///
/// Walks the AST to find which key or value contains `offset`. Returns the JSON
/// pointer path as a list of segments and the byte range of the hit node (for
/// hover highlighting). Returns `None` if the offset falls on structural tokens,
/// whitespace, or outside the AST.
pub fn offset_to_pointer(ast: &AstValue, offset: usize) -> Option<(Vec<String>, Range<usize>)> {
    let r = ast.range();
    if offset < r.start || offset >= r.end {
        return None;
    }
    let mut path: Vec<String> = Vec::new();
    offset_to_pointer_walk(ast, offset, &mut path)
}

fn offset_to_pointer_walk(
    node: &AstValue,
    offset: usize,
    path: &mut Vec<String>,
) -> Option<(Vec<String>, Range<usize>)> {
    match node {
        AstValue::Object(obj) => {
            for prop in &obj.properties {
                // Check if offset is on the key.
                let key_range = prop.name.range();
                if offset >= key_range.start && offset < key_range.end {
                    path.push(prop.name.as_str().to_string());
                    return Some((path.clone(), key_range.start..key_range.end));
                }
                // Check if offset is on the value.
                let val_range = prop.value.range();
                if offset >= val_range.start && offset < val_range.end {
                    path.push(prop.name.as_str().to_string());
                    return offset_to_pointer_walk(&prop.value, offset, path);
                }
            }
            None // offset is on structural tokens ({, }, :, ,) or whitespace
        }
        AstValue::Array(arr) => {
            for (idx, elem) in arr.elements.iter().enumerate() {
                let elem_range = elem.range();
                if offset >= elem_range.start && offset < elem_range.end {
                    path.push(idx.to_string());
                    return offset_to_pointer_walk(elem, offset, path);
                }
            }
            None
        }
        // Leaf nodes: the offset is within this scalar value.
        _ => {
            let r = node.range();
            Some((path.clone(), r.start..r.end))
        }
    }
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

    #[test]
    fn test_offset_to_pointer_on_key() {
        // {"name": "value"}
        let source = r#"{"name": "value"}"#;
        let parsed = parse_jsonc(source).unwrap();
        // "name" starts at byte 1 (the opening quote)
        let result = offset_to_pointer(&parsed.ast, 1);
        assert!(result.is_some());
        let (pointer, _range) = result.unwrap();
        assert_eq!(pointer, vec!["name"]);
    }

    #[test]
    fn test_offset_to_pointer_on_value() {
        let source = r#"{"name": "value"}"#;
        let parsed = parse_jsonc(source).unwrap();
        // "value" starts at byte 9 (the opening quote of the value)
        let result = offset_to_pointer(&parsed.ast, 9);
        assert!(result.is_some());
        let (pointer, _range) = result.unwrap();
        assert_eq!(pointer, vec!["name"]);
    }

    #[test]
    fn test_offset_to_pointer_nested() {
        let source = r#"{"a": {"b": 42}}"#;
        let parsed = parse_jsonc(source).unwrap();
        // 42 is the value at pointer ["a", "b"]
        // Find offset of "42" in the source
        let offset = source.find("42").unwrap();
        let result = offset_to_pointer(&parsed.ast, offset);
        assert!(result.is_some());
        let (pointer, _range) = result.unwrap();
        assert_eq!(pointer, vec!["a", "b"]);
    }

    #[test]
    fn test_offset_to_pointer_on_structural_token() {
        let source = r#"{"name": "value"}"#;
        let parsed = parse_jsonc(source).unwrap();
        // Offset 0 is the opening brace `{`
        let result = offset_to_pointer(&parsed.ast, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_offset_to_pointer_array_element() {
        let source = r#"{"items": [1, 2, 3]}"#;
        let parsed = parse_jsonc(source).unwrap();
        // Find offset of "2" in the source
        let offset = source.find(", 2").unwrap() + 2;
        let result = offset_to_pointer(&parsed.ast, offset);
        assert!(result.is_some());
        let (pointer, _range) = result.unwrap();
        assert_eq!(pointer, vec!["items", "1"]);
    }

    #[test]
    fn test_offset_to_pointer_out_of_range() {
        let source = r#"{"key": "val"}"#;
        let parsed = parse_jsonc(source).unwrap();
        let result = offset_to_pointer(&parsed.ast, 999);
        assert!(result.is_none());
    }
}
