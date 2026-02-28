mod common;

use std::time::Duration;

use common::lsp_client::{TestClient, file_uri};

fn simple_schema_path() -> String {
    format!(
        "{}/tests/fixtures/simple-schema.json",
        env!("CARGO_MANIFEST_DIR")
    )
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
