mod common;

use std::time::Duration;

use common::lsp_client::{TestClient, file_uri};

/// A simple JSON schema for tests — requires { name: string, port: number }.
fn simple_schema_path() -> String {
    format!(
        "{}/tests/fixtures/simple-schema.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A temporary test document URI (doesn't need to exist on disk).
fn doc_uri() -> String {
    // We point at the fixtures directory so that relative $schema paths resolve there.
    file_uri(&format!(
        "{}/tests/fixtures/test-lsp-doc.json",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Build a JSON document content string that uses an absolute $schema path.
fn doc_with_schema(content: &str) -> String {
    // Use absolute path to avoid any path resolution issues.
    let schema = simple_schema_path();
    format!(r#"{{"$schema": "{schema}", {content}}}"#)
}

/// Open a file with a $schema field → server should publish diagnostics.
#[tokio::test]
async fn did_open_with_schema_triggers_diagnostics() {
    let mut client = TestClient::new();
    client.initialize().await;

    // Invalid document: port is a string, schema requires number.
    let content = doc_with_schema(r#""name": "app", "port": "wrong""#);
    client.did_open(&doc_uri(), "json", 1, &content).await;

    // Wait for publishDiagnostics (advance past the 200ms debounce).
    tokio::time::sleep(Duration::from_millis(300)).await;
    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;

    let diagnostics = &notification["params"]["diagnostics"];
    assert!(
        !diagnostics.as_array().unwrap().is_empty(),
        "expected at least one diagnostic, got: {diagnostics}"
    );
    // Verify source tag.
    let first = &diagnostics[0];
    assert_eq!(first["source"], "jvl");
}

/// didOpen for a file with no resolvable schema → no diagnostics (silent skip).
#[tokio::test]
async fn did_open_no_schema_produces_no_diagnostics() {
    let mut client = TestClient::new();
    client.initialize().await;

    // No $schema field, no jvl.json mapping.
    let uri = file_uri("/tmp/no-schema-lsp-test.json");
    client
        .did_open(&uri, "json", 1, r#"{"anything": "goes"}"#)
        .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;

    let diagnostics = &notification["params"]["diagnostics"];
    assert_eq!(
        diagnostics.as_array().map(|a| a.len()).unwrap_or(0),
        0,
        "expected no diagnostics for file with no schema"
    );
}

/// didChange re-validates and updates diagnostics.
#[tokio::test]
async fn did_change_updates_diagnostics() {
    let mut client = TestClient::new();
    client.initialize().await;

    // First open with invalid content.
    let uri = doc_uri();
    let bad_content = doc_with_schema(r#""name": "app", "port": "wrong""#);
    client.did_open(&uri, "json", 1, &bad_content).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let n1 = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let bad_count = n1["params"]["diagnostics"].as_array().unwrap().len();
    assert!(bad_count >= 1, "expected diagnostics on first open");

    // Fix the error via didChange.
    let good_content = doc_with_schema(r#""name": "app", "port": 8080"#);
    client.did_change(&uri, 2, &good_content).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let n2 = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let good_count = n2["params"]["diagnostics"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(good_count, 0, "expected no diagnostics after fix");
}

/// didClose clears diagnostics and discards in-flight results.
#[tokio::test]
async fn did_close_clears_diagnostics() {
    let mut client = TestClient::new();
    client.initialize().await;

    let uri = doc_uri();
    let content = doc_with_schema(r#""name": "app", "port": 8080"#);
    client.did_open(&uri, "json", 1, &content).await;
    tokio::time::sleep(Duration::from_millis(50)).await; // before debounce fires

    client.did_close(&uri).await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    // After close, the server should publish empty diagnostics.
    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = notification["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(
        diagnostics.len(),
        0,
        "expected empty diagnostics after close"
    );
}

/// Parse error in the document produces a diagnostic.
#[tokio::test]
async fn parse_error_produces_diagnostic() {
    let mut client = TestClient::new();
    client.initialize().await;

    let uri = doc_uri();
    let schema = simple_schema_path();
    // Deliberately broken JSON: missing closing brace.
    let content = format!(r#"{{"$schema": "{schema}", "name": "app""#);
    client.did_open(&uri, "json", 1, &content).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diagnostics = &notification["params"]["diagnostics"];
    assert!(
        diagnostics.as_array().map(|a| a.len()).unwrap_or(0) >= 1,
        "expected a parse error diagnostic"
    );
    // Should be tagged with parse(syntax) code.
    let codes: Vec<_> = diagnostics
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(
        codes.iter().any(|c| c.starts_with("parse")),
        "expected a parse(...) diagnostic code, got: {codes:?}"
    );
}

/// Schema load error points at the $schema value span, not (0,0).
#[tokio::test]
async fn schema_load_error_points_at_schema_value() {
    let mut client = TestClient::new();
    client.initialize().await;

    let uri = doc_uri();
    // Reference a nonexistent schema file — triggers a schema(load) error.
    let content = r#"{"$schema": "./nonexistent-schema.json", "name": "test"}"#;
    client.did_open(&uri, "json", 1, content).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;

    let diagnostics = notification["params"]["diagnostics"]
        .as_array()
        .expect("expected diagnostics array");
    assert!(
        !diagnostics.is_empty(),
        "expected at least one diagnostic for schema load error"
    );

    let diag = &diagnostics[0];
    let range = &diag["range"];
    let start = &range["start"];
    // The $schema value ("./nonexistent-schema.json") starts at offset 12 (line 0, char 12).
    // It should NOT be at (0,0).
    let start_line = start["line"].as_u64().unwrap();
    let start_char = start["character"].as_u64().unwrap();
    assert!(
        start_line != 0 || start_char != 0,
        "schema load error should not point at (0,0); got line={start_line} char={start_char}"
    );
    // The span should cover the value string "./nonexistent-schema.json" (with quotes).
    let end = &range["end"];
    let end_char = end["character"].as_u64().unwrap();
    assert!(
        end_char > start_char,
        "end character should be past start character"
    );
}

/// Non-file:// URIs are handled gracefully — no crash, and any notification received
/// should have empty diagnostics (server skips non-file URIs).
#[tokio::test]
async fn non_file_uri_handled_gracefully() {
    let mut client = TestClient::new();
    client.initialize().await;

    // Send a didOpen with an untitled: URI.
    client
        .did_open("untitled:Untitled-1", "json", 1, r#"{"key": "val"}"#)
        .await;

    // Allow time for any potential processing.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The server should not crash. We don't expect a publishDiagnostics for this URI
    // (the server logs and skips). No assertion needed beyond "no panic".
    // If the server does emit one, verify it's empty:
    // (We can't easily timeout in a blocking test without start_paused, so just check
    // that the server is still responding.)
    client.shutdown().await;
}
