mod common;

use std::time::Duration;

use common::lsp_client::{TestClient, file_uri};

/// Absolute path to the hover test schema.
fn hover_schema_path() -> String {
    format!(
        "{}/tests/fixtures/hover-schema.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A test document URI in the fixtures directory (so relative $schema resolves).
fn doc_uri() -> String {
    file_uri(&format!(
        "{}/tests/fixtures/test-hover-doc.json",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Build a JSON document with an absolute $schema pointing to hover-schema.json.
fn doc_with_hover_schema(content: &str) -> String {
    let schema = hover_schema_path();
    format!(r#"{{"$schema": "{schema}", {content}}}"#)
}

/// Open a document and wait for validation to complete (past the debounce window).
async fn open_and_wait(client: &mut TestClient, uri: &str, content: &str) {
    client.did_open(uri, "json", 1, content).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Drain the diagnostics notification.
    client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
}

/// Hovering on a property key shows title and description from the schema.
#[tokio::test]
async fn hover_on_key_shows_title_and_description() {
    let mut client = TestClient::new();
    client.initialize().await;

    // {"$schema": "...", "name": "Alice"}
    let content = doc_with_hover_schema(r#""name": "Alice""#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // "name" key starts after {"$schema": "...", — find the position of the key.
    let name_offset = content.find(r#""name""#).unwrap();
    let line = 0u32;
    let character = name_offset as u32;

    let result = client.hover(&uri, line, character).await;
    assert!(!result.is_null(), "expected hover result, got null");
    let value = result["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("**Name**"),
        "expected title in hover, got: {value}"
    );
    assert!(
        value.contains("The user's full name"),
        "expected description in hover, got: {value}"
    );
}

/// Hovering on a property value shows the same annotation as hovering on its key.
#[tokio::test]
async fn hover_on_value_shows_same_annotation() {
    let mut client = TestClient::new();
    client.initialize().await;

    let content = doc_with_hover_schema(r#""name": "Alice""#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Hover on the value "Alice".
    let value_offset = content.find(r#""Alice""#).unwrap();
    let result = client.hover(&uri, 0, value_offset as u32).await;
    assert!(!result.is_null(), "expected hover result, got null");
    let value = result["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("**Name**"),
        "expected title in hover on value, got: {value}"
    );
}

/// Hovering on a nested property resolves through multiple properties levels.
#[tokio::test]
async fn hover_on_nested_property() {
    let mut client = TestClient::new();
    client.initialize().await;

    let content = doc_with_hover_schema(r#""settings": {"theme": "dark"}"#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Hover on "theme" key inside settings.
    let theme_offset = content.find(r#""theme""#).unwrap();
    let result = client.hover(&uri, 0, theme_offset as u32).await;
    assert!(!result.is_null(), "expected hover result, got null");
    let value = result["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("**Theme**"),
        "expected Theme title, got: {value}"
    );
    assert!(
        value.contains("UI color theme"),
        "expected theme description, got: {value}"
    );
}

/// Hovering on a property resolved through $ref shows the referenced schema's annotations.
#[tokio::test]
async fn hover_on_ref_resolved_property() {
    let mut client = TestClient::new();
    client.initialize().await;

    let content = doc_with_hover_schema(r#""address": {"street": "123 Main St"}"#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Hover on "street" inside address (which is $ref -> Address).
    let street_offset = content.find(r#""street""#).unwrap();
    let result = client.hover(&uri, 0, street_offset as u32).await;
    assert!(!result.is_null(), "expected hover result, got null");
    let value = result["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("**Street**"),
        "expected Street title, got: {value}"
    );
}

/// Hovering on a property with no title/description returns null.
#[tokio::test]
async fn hover_no_annotation_returns_null() {
    let mut client = TestClient::new();
    client.initialize().await;

    // "age" has type: number but no title/description.
    let content = doc_with_hover_schema(r#""age": 30"#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    let age_offset = content.find(r#""age""#).unwrap();
    let result = client.hover(&uri, 0, age_offset as u32).await;
    assert!(
        result.is_null(),
        "expected null for property without annotations"
    );
}

/// Hovering in a file with no schema returns null.
#[tokio::test]
async fn hover_no_schema_returns_null() {
    let mut client = TestClient::new();
    client.initialize().await;

    let uri = file_uri("/tmp/no-schema-hover-test.json");
    client
        .did_open(&uri, "json", 1, r#"{"anything": "goes"}"#)
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    client
        .recv_notification("textDocument/publishDiagnostics")
        .await;

    let result = client.hover(&uri, 0, 1).await;
    assert!(
        result.is_null(),
        "expected null when no schema is available"
    );
}
