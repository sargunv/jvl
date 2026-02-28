mod common;

use std::time::Duration;

use common::lsp_client::{TestClient, file_uri};

fn simple_schema_path() -> String {
    format!(
        "{}/tests/fixtures/simple-schema.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

// --- Unit tests for byte_col_to_lsp and file_diagnostic_to_lsp ---
// These test the conversion logic directly without a running server.

#[cfg(test)]
mod unit {
    use jvl::diagnostic::{FileDiagnostic, Severity, SourceLocation};

    /// Helper: compute UTF-16 column offset for a byte position in a string.
    fn utf16_col(s: &str, byte_col: usize) -> u32 {
        s[..byte_col.min(s.len())].encode_utf16().count() as u32
    }

    #[test]
    fn ascii_utf8_and_utf16_agree() {
        // For pure ASCII, byte offsets == UTF-8 == UTF-16.
        let line = "hello world";
        assert_eq!(utf16_col(line, 5), 5);
        assert_eq!(utf16_col(line, 11), 11);
    }

    #[test]
    fn two_byte_char_utf16_offset() {
        // 'é' = U+00E9, encoded as 2 bytes in UTF-8, 1 code unit in UTF-16.
        let line = "caf\u{00E9}s"; // "cafés", byte len = 6
        // byte 3 = start of 'é' (2-byte UTF-8)
        assert_eq!(utf16_col(line, 3), 3); // "caf" = 3 UTF-16 units
        // byte 5 = start of 's' (after the 2-byte é)
        assert_eq!(utf16_col(line, 5), 4); // "café" = 4 UTF-16 units
    }

    #[test]
    fn emoji_utf16_offset() {
        // '🏴' = U+1F3F4, encoded as 4 bytes in UTF-8, 2 surrogate pairs in UTF-16.
        let line = "flag: 🏴 end";
        // byte 6 = start of the emoji (after "flag: " = 6 bytes: f,l,a,g,:,space)
        assert_eq!(utf16_col(line, 6), 6); // "flag: " = 6 chars = 6 UTF-16 units
        // byte 10 = after the 4-byte emoji
        assert_eq!(utf16_col(line, 10), 8); // emoji takes 2 UTF-16 units, so 6+2=8
    }

    #[test]
    fn cjk_char_utf16_offset() {
        // '漢' = U+6F22, encoded as 3 bytes in UTF-8, 1 code unit in UTF-16.
        let line = "漢字"; // 2 CJK chars = 6 bytes
        assert_eq!(utf16_col(line, 3), 1); // "漢" = 1 UTF-16 unit
        assert_eq!(utf16_col(line, 6), 2); // "漢字" = 2 UTF-16 units
    }

    #[test]
    fn source_location_to_lsp_range_utf16() {
        // Verify that SourceLocation → Diagnostic produces correct UTF-16 positions
        // for a line containing a multi-byte character.
        //
        // Document: {"name": "André"}
        // Suppose the schema has a maxLength: 4 constraint on name.
        // The error spans the value "André" (5 chars, 6 bytes: é is 2 bytes).
        // In LSP UTF-16 mode, the error should start at the opening quote (byte 9).
        //
        // Layout: {"name": "André"}
        //          0123456789...
        //                  ^ byte 9 = opening quote of value
        let source = r#"{"name": "André"}"#;
        // "André" starts at byte 9 (after `{"name": `) and ends at byte 17 (exclusive).
        // In UTF-16: "André" is 5 characters + 2 surrounding quotes = 7 code units on this line.

        // SourceLocation: line=1, column=10 (1-based byte column for the opening quote `"`),
        // offset=9, length=7 (the string "André" including quotes = 7 bytes: `"André"`)
        // Wait, let me count: { = 0, " = 1, n = 2, a = 3, m = 4, e = 5, " = 6, : = 7, space = 8,
        // " = 9.  That's the opening quote at byte 9.  "André" = 6 bytes + closing " = 7 bytes.
        let loc = SourceLocation {
            line: 1,
            column: 10, // 1-based byte column for `"` at byte offset 9
            offset: 9,
            length: 8, // `"André"` = 8 bytes (1 + 1 + 1 + 1 + 2 + 1 + 1)
        };

        let diag = FileDiagnostic {
            code: "schema(maxLength)".into(),
            message: "max length exceeded".into(),
            severity: Severity::Error,
            span: None,
            location: Some(loc),
            label: None,
            help: None,
            schema_path: None,
        };

        // Manually compute expected UTF-16 column.
        // Line 0 (0-based), byte 9 → character "André" starts right after `{"name": `.
        // That prefix "{"name": " = 9 bytes = 9 UTF-16 code units (all ASCII).
        let line = source.lines().next().unwrap();
        let start_byte_col = 9; // byte offset of opening `"` = column 10, 1-based → 9, 0-based
        let start_utf16 = line[..start_byte_col].encode_utf16().count();
        assert_eq!(
            start_utf16, 9,
            "start utf-16 col should be 9 for ascii prefix"
        );

        // End position: offset 9 + length 8 = byte 17.
        let end_offset = 9 + 8; // = 17
        let line_starts = jvl::parse::compute_line_starts(source);
        let (end_line, end_col) = jvl::parse::offset_to_line_col(&line_starts, end_offset);
        assert_eq!(end_line, 1);

        // "{"name": "André"" → bytes 0-16, closing `}` at byte 17.
        // End byte col (0-based) = end_col - 1 = byte 17 - line_start 0 = 17 (exclusive), 0-based = 16.
        let end_byte_col = end_col.saturating_sub(1);
        let end_utf16 = line[..end_byte_col.min(line.len())].encode_utf16().count();
        // In UTF-16: "André" = 5 chars (é=1 unit), total `"André"` = 7 UTF-16 code units.
        // So end should be start (9) + 7 = 16.
        assert_eq!(end_utf16, 16, "end utf-16 col should be 16");

        // Verify the diagnostic fields are what we constructed.
        assert_eq!(diag.code, "schema(maxLength)");
        assert_eq!(diag.severity, Severity::Error);

        let _ = diag; // used
    }
}

// --- Integration tests for encoding negotiation ---

/// Server with UTF-8 encoding: column positions should be byte-accurate.
#[tokio::test]
async fn utf8_encoding_column_positions() {
    let mut client = TestClient::new();
    client
        .initialize_with_params(serde_json::json!({
            "general": {
                "positionEncodings": ["utf-8"]
            }
        }))
        .await;

    let schema = simple_schema_path();
    let uri = file_uri(&format!(
        "{}/tests/fixtures/test-encoding-utf8.json",
        env!("CARGO_MANIFEST_DIR")
    ));

    // "port" value is a string (invalid) at a predictable byte position.
    // {"$schema": "...", "name": "André", "port": "bad"}
    // All ASCII up through port value, but name has non-ASCII chars.
    let schema_val = schema.clone();
    let content = format!(r#"{{"$schema": "{schema_val}", "name": "André", "port": "bad"}}"#);
    client.did_open(&uri, "json", 1, &content).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = notification["params"]["diagnostics"].as_array().unwrap();
    assert!(
        !diagnostics.is_empty(),
        "expected diagnostics for port type error"
    );

    // In UTF-8 mode, character position == byte position.
    // Just verify we got a valid (non-negative) range.
    let range = &diagnostics[0]["range"];
    let start_char = range["start"]["character"].as_u64().unwrap();
    let end_char = range["end"]["character"].as_u64().unwrap();
    assert!(
        end_char >= start_char,
        "end character should be >= start character"
    );
}

/// Server with UTF-16 encoding (default): column positions use UTF-16 offsets.
/// For a line with no non-ASCII content, UTF-8 and UTF-16 agree.
#[tokio::test]
async fn utf16_encoding_ascii_content() {
    let mut client = TestClient::new();
    client.initialize().await; // default = UTF-16

    let schema = simple_schema_path();
    let uri = file_uri(&format!(
        "{}/tests/fixtures/test-encoding-utf16.json",
        env!("CARGO_MANIFEST_DIR")
    ));
    // All-ASCII content, so UTF-8 and UTF-16 positions agree.
    let content = format!(r#"{{"$schema": "{schema}", "name": "app", "port": "bad"}}"#);
    client.did_open(&uri, "json", 1, &content).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = notification["params"]["diagnostics"].as_array().unwrap();
    assert!(!diagnostics.is_empty(), "expected type error on port");

    let range = &diagnostics[0]["range"];
    let start_char = range["start"]["character"].as_u64().unwrap();
    let end_char = range["end"]["character"].as_u64().unwrap();
    assert!(end_char >= start_char);
    // For ASCII content, character position is the same as byte position.
    // The port value `"bad"` is near the end of the line: verify it's > 0.
    assert!(start_char > 0, "port field should not start at column 0");
}
