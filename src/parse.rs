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

/// Result of analyzing cursor context for completions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// Cursor is in a position where a property key should go.
    PropertyKey {
        /// JSON pointer path to the containing object (e.g., `["server"]` for
        /// a cursor inside the `"server"` object). Empty for the root object.
        pointer: Vec<String>,
    },
    /// Cursor is in a position where a property value should go.
    PropertyValue {
        /// The property name whose value is being completed.
        property_name: String,
        /// JSON pointer path to the containing object.
        pointer: Vec<String>,
    },
}

/// Determine the completion context at a byte offset by scanning the source text.
///
/// Scans forward from the start of the document, tracking string boundaries,
/// comments, and brace nesting to determine whether the cursor is at a property
/// key or value position within a JSON object.
///
/// Returns `None` if the cursor is inside a comment, inside an array (not a
/// nested object), or at the document root outside any object.
pub fn completion_context(source: &str, byte_offset: usize) -> Option<CompletionContext> {
    let end = byte_offset.min(source.len());
    let bytes = source.as_bytes();

    // Scanner state.
    let mut in_string = false;
    let mut escape = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    // Stack tracking container nesting.
    struct Level {
        is_object: bool,
        /// True after `:` is seen, cleared by `,`.
        after_colon: bool,
        /// Byte range (start, end exclusive) of the last completed key string
        /// at this level (content inside the quotes, not including quotes).
        last_key_content: Option<(usize, usize)>,
        /// Key from the parent level whose value is this container.
        parent_key: Option<String>,
    }
    let mut stack: Vec<Level> = Vec::new();

    // Track the start of the current string (byte after the opening `"`).
    let mut string_content_start: usize = 0;

    let mut i = 0;
    while i < end {
        let b = bytes[i];

        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }

        if in_block_comment {
            if b == b'*' && i + 1 < end && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if in_string {
            if escape {
                escape = false;
                i += 1;
                continue;
            }
            if b == b'\\' {
                escape = true;
                i += 1;
                continue;
            }
            if b == b'"' {
                // String closed.
                if let Some(level) = stack.last_mut()
                    && level.is_object
                {
                    if !level.after_colon {
                        // This string was a property key.
                        level.last_key_content = Some((string_content_start, i));
                    } else {
                        // This string was a property value; value consumed.
                        level.after_colon = false;
                    }
                }
                in_string = false;
            }
            i += 1;
            continue;
        }

        // Not in string, not in comment.
        match b {
            b'/' if i + 1 < end => match bytes[i + 1] {
                b'/' => {
                    in_line_comment = true;
                    i += 2;
                    continue;
                }
                b'*' => {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
                _ => {}
            },
            b'"' => {
                in_string = true;
                string_content_start = i + 1;
            }
            b'{' => {
                // Determine the parent key for this new object.
                let parent_key = stack.last().and_then(|parent| {
                    if parent.is_object && parent.after_colon {
                        parent
                            .last_key_content
                            .map(|(s, e)| source[s..e].to_string())
                    } else {
                        None
                    }
                });
                stack.push(Level {
                    is_object: true,
                    after_colon: false,
                    last_key_content: None,
                    parent_key,
                });
            }
            b'[' => {
                stack.push(Level {
                    is_object: false,
                    after_colon: false,
                    last_key_content: None,
                    parent_key: None,
                });
            }
            b'}' | b']' => {
                stack.pop();
                // After closing a nested container that was a value, reset the
                // parent's after_colon so the parent is back in key position.
                if let Some(level) = stack.last_mut()
                    && level.is_object
                    && level.after_colon
                {
                    level.after_colon = false;
                }
            }
            b':' => {
                if let Some(level) = stack.last_mut()
                    && level.is_object
                {
                    level.after_colon = true;
                }
            }
            b',' => {
                if let Some(level) = stack.last_mut() {
                    level.after_colon = false;
                    if level.is_object {
                        level.last_key_content = None;
                    }
                }
            }
            _ => {}
        }

        i += 1;
    }

    // Determine the context at the cursor position.
    if in_line_comment || in_block_comment {
        return None;
    }

    // Build the pointer path from the stack (skip root level, collect parent keys).
    let build_pointer = |stack: &[Level]| -> Vec<String> {
        stack
            .iter()
            .skip(1)
            .filter_map(|l| l.parent_key.clone())
            .collect()
    };

    // Find the innermost object level (skip any array levels on top).
    let innermost_object = stack.iter().rposition(|l| l.is_object);

    if in_string {
        // Cursor is inside a string.
        let obj_idx = innermost_object?;
        let level = &stack[obj_idx];
        let pointer = build_pointer(&stack[..=obj_idx]);
        if level.after_colon {
            let property_name = level
                .last_key_content
                .map(|(s, e)| source[s..e].to_string())
                .unwrap_or_default();
            Some(CompletionContext::PropertyValue {
                property_name,
                pointer,
            })
        } else {
            Some(CompletionContext::PropertyKey { pointer })
        }
    } else {
        // Cursor is on whitespace or structural tokens.
        let obj_idx = innermost_object?;
        let level = &stack[obj_idx];
        let pointer = build_pointer(&stack[..=obj_idx]);
        if level.after_colon {
            let property_name = level
                .last_key_content
                .map(|(s, e)| source[s..e].to_string())
                .unwrap_or_default();
            Some(CompletionContext::PropertyValue {
                property_name,
                pointer,
            })
        } else {
            Some(CompletionContext::PropertyKey { pointer })
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

    // --- completion_context tests ---

    #[test]
    fn completion_context_empty_object() {
        // {|}
        let source = "{}";
        let result = completion_context(source, 1); // between { and }
        assert_eq!(
            result,
            Some(CompletionContext::PropertyKey { pointer: vec![] })
        );
    }

    #[test]
    fn completion_context_after_comma() {
        // {"a": 1, |}
        let source = r#"{"a": 1, }"#;
        let result = completion_context(source, 9); // after ", " before }
        assert_eq!(
            result,
            Some(CompletionContext::PropertyKey { pointer: vec![] })
        );
    }

    #[test]
    fn completion_context_after_colon() {
        // {"name": |}
        let source = r#"{"name": }"#;
        let result = completion_context(source, 9); // after ": " before }
        assert_eq!(
            result,
            Some(CompletionContext::PropertyValue {
                property_name: "name".into(),
                pointer: vec![],
            })
        );
    }

    #[test]
    fn completion_context_inside_key_string() {
        // {"na|}
        let source = r#"{"na"#;
        let result = completion_context(source, 3); // inside "na
        assert_eq!(
            result,
            Some(CompletionContext::PropertyKey { pointer: vec![] })
        );
    }

    #[test]
    fn completion_context_inside_value_string() {
        // {"name": "Al|}
        let source = r#"{"name": "Al"#;
        let result = completion_context(source, 11); // inside "Al
        assert_eq!(
            result,
            Some(CompletionContext::PropertyValue {
                property_name: "name".into(),
                pointer: vec![],
            })
        );
    }

    #[test]
    fn completion_context_nested_object() {
        // {"server": {|}}
        let source = r#"{"server": {}}"#;
        let result = completion_context(source, 12); // inside nested {}
        assert_eq!(
            result,
            Some(CompletionContext::PropertyKey {
                pointer: vec!["server".into()],
            })
        );
    }

    #[test]
    fn completion_context_nested_value() {
        // {"server": {"host": |}}
        let source = r#"{"server": {"host": }}"#;
        let result = completion_context(source, 20); // after "host":
        assert_eq!(
            result,
            Some(CompletionContext::PropertyValue {
                property_name: "host".into(),
                pointer: vec!["server".into()],
            })
        );
    }

    #[test]
    fn completion_context_inside_comment() {
        let source = "{ // |\n}";
        let result = completion_context(source, 5); // inside line comment
        assert!(result.is_none());
    }

    #[test]
    fn completion_context_inside_block_comment() {
        let source = "{ /* | */ }";
        let result = completion_context(source, 5); // inside block comment
        assert!(result.is_none());
    }

    #[test]
    fn completion_context_document_root() {
        let source = "  ";
        let result = completion_context(source, 1); // outside any container
        assert!(result.is_none());
    }

    #[test]
    fn completion_context_just_open_brace() {
        // Malformed: just {
        let source = "{";
        let result = completion_context(source, 1);
        assert_eq!(
            result,
            Some(CompletionContext::PropertyKey { pointer: vec![] })
        );
    }

    #[test]
    fn completion_context_after_colon_no_value() {
        // Malformed: {"name":
        let source = r#"{"name":"#;
        let result = completion_context(source, 8);
        assert_eq!(
            result,
            Some(CompletionContext::PropertyValue {
                property_name: "name".into(),
                pointer: vec![],
            })
        );
    }
}
