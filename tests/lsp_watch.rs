mod common;

use std::time::Duration;

use common::lsp_client::{TestClient, file_uri};

/// Schema change on disk → re-validate open documents and update diagnostics.
///
/// Setup: write a schema that requires `{"name": string}` to a tempdir,
/// open a document referencing it with `"name": 123` (invalid), confirm we get
/// a diagnostic. Then change the schema on disk to require `{"name": number}`,
/// notify via `didChangeWatchedFiles`, and confirm the diagnostic clears.
#[tokio::test]
async fn schema_file_change_triggers_revalidation() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    let doc_path = dir.path().join("test.json");

    // Schema v1: requires { name: string }.
    std::fs::write(
        &schema_path,
        r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#,
    )
    .unwrap();

    let schema_abs = std::fs::canonicalize(&schema_path).unwrap();
    let schema_uri = file_uri(&schema_abs.display().to_string());

    // Document: name is a number → invalid against v1 schema.
    let doc_content = format!(r#"{{"$schema": "{}", "name": 123}}"#, schema_abs.display());
    let doc_uri = file_uri(&doc_path.display().to_string());

    let mut client = TestClient::new();
    client.initialize().await;

    // Open the document; should produce diagnostics (name is number, not string).
    client.did_open(&doc_uri, "json", 1, &doc_content).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let n1 = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diags1 = n1["params"]["diagnostics"].as_array().unwrap();
    assert!(
        !diags1.is_empty(),
        "expected diagnostics when name is number but schema requires string"
    );

    // Now change the schema to accept { name: number }.
    std::fs::write(
        &schema_path,
        r#"{"type":"object","properties":{"name":{"type":"number"}},"required":["name"]}"#,
    )
    .unwrap();

    // Notify the server that the schema file changed (type 2 = Changed).
    client.did_change_watched_files(&[(&schema_uri, 2)]).await;

    // Wait for re-validation.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let n2 = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diags2 = n2["params"]["diagnostics"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(
        diags2, 0,
        "expected no diagnostics after schema changed to accept number"
    );

    client.shutdown().await;
}

/// Schema file deleted → re-validate produces a schema load error.
///
/// Open a document referencing a local schema, confirm it validates, then delete
/// the schema file and notify via `didChangeWatchedFiles`. The server should
/// publish a schema(load) diagnostic.
#[tokio::test]
async fn schema_file_deleted_triggers_load_error() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    let doc_path = dir.path().join("test.json");

    // Schema: accepts anything (empty schema = allow all).
    std::fs::write(&schema_path, r#"{"type":"object"}"#).unwrap();

    let schema_abs = std::fs::canonicalize(&schema_path).unwrap();
    let schema_uri = file_uri(&schema_abs.display().to_string());

    // Document referencing the schema.
    let doc_content = format!(r#"{{"$schema": "{}"}}"#, schema_abs.display());
    let doc_uri = file_uri(&doc_path.display().to_string());

    let mut client = TestClient::new();
    client.initialize().await;

    // Open document; should validate successfully (no errors).
    client.did_open(&doc_uri, "json", 1, &doc_content).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let n1 = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diags1 = n1["params"]["diagnostics"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(diags1, 0, "expected no diagnostics with valid schema");

    // Delete the schema file.
    std::fs::remove_file(&schema_path).unwrap();

    // Notify the server that the schema file was deleted (type 3 = Deleted).
    client.did_change_watched_files(&[(&schema_uri, 3)]).await;

    // Wait for re-validation.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let n2 = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let diags2 = n2["params"]["diagnostics"].as_array().unwrap();
    assert!(
        !diags2.is_empty(),
        "expected diagnostics after schema file deleted"
    );

    // Should be a schema load error.
    let code = diags2[0]["code"].as_str().unwrap_or("");
    assert!(
        code.starts_with("schema("),
        "expected schema error code, got: {code}"
    );

    client.shutdown().await;
}
