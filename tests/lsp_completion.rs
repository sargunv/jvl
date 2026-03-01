mod common;

use std::time::Duration;

use common::lsp_client::{TestClient, file_uri};

/// Absolute path to the completion test schema.
fn completion_schema_path() -> String {
    format!(
        "{}/tests/fixtures/completion-schema.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A test document URI in the fixtures directory (so relative $schema resolves).
fn doc_uri() -> String {
    file_uri(&format!(
        "{}/tests/fixtures/test-completion-doc.json",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Build a JSON document with an absolute $schema pointing to completion-schema.json.
fn doc_with_schema(content: &str) -> String {
    let schema = completion_schema_path();
    if content.is_empty() {
        format!(r#"{{"$schema": "{schema}"}}"#)
    } else {
        format!(r#"{{"$schema": "{schema}", {content}}}"#)
    }
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

/// Extract completion item labels from a completion result.
fn labels(result: &serde_json::Value) -> Vec<String> {
    result["items"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|item| item["label"].as_str().unwrap().to_string())
        .collect()
}

/// Property name completion in a nearly-empty object.
#[tokio::test]
async fn completion_property_names_in_empty_object() {
    let mut client = TestClient::new();
    client.initialize().await;

    // {"$schema": "...", }
    //                   ^ cursor here (after the comma, before })
    let content = doc_with_schema("");
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Position cursor after "$schema": "...", — find the comma after the schema value
    // The content is: {"$schema": "/path/to/schema.json"}
    // Cursor at the position just before }
    let cursor_col = (content.len() - 1) as u32; // before closing }
    let result = client.completion(&uri, 0, cursor_col).await;

    let names = labels(&result);
    assert!(
        names.contains(&"name".to_string()),
        "expected 'name' in completions, got: {names:?}"
    );
    assert!(
        names.contains(&"age".to_string()),
        "expected 'age' in completions, got: {names:?}"
    );
    assert!(
        names.contains(&"enabled".to_string()),
        "expected 'enabled' in completions, got: {names:?}"
    );
    // $schema should be filtered (it's already present)
    assert!(
        !names.contains(&"$schema".to_string()),
        "'$schema' should be filtered from completions"
    );
}

/// Required properties sort before optional properties.
#[tokio::test]
async fn completion_required_properties_sort_first() {
    let mut client = TestClient::new();
    client.initialize().await;

    let content = doc_with_schema("");
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    let cursor_col = (content.len() - 1) as u32;
    let result = client.completion(&uri, 0, cursor_col).await;

    let items = result["items"].as_array().unwrap();
    let name_item = items.iter().find(|i| i["label"] == "name").unwrap();
    let age_item = items.iter().find(|i| i["label"] == "age").unwrap();

    let name_sort = name_item["sortText"].as_str().unwrap();
    let age_sort = age_item["sortText"].as_str().unwrap();

    assert!(
        name_sort < age_sort,
        "required property 'name' should sort before optional 'age': {name_sort} vs {age_sort}"
    );

    // Required properties have detail with "(required)"
    let name_detail = name_item["detail"].as_str().unwrap();
    assert!(
        name_detail.contains("required"),
        "expected '(required)' in detail for 'name', got: {name_detail}"
    );
}

/// Already-present properties are filtered from suggestions.
#[tokio::test]
async fn completion_filters_existing_properties() {
    let mut client = TestClient::new();
    client.initialize().await;

    // {"$schema": "...", "name": "Alice", }
    let content = doc_with_schema(r#""name": "Alice""#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    let cursor_col = (content.len() - 1) as u32;
    let result = client.completion(&uri, 0, cursor_col).await;

    let names = labels(&result);
    assert!(
        !names.contains(&"name".to_string()),
        "'name' should be filtered since it's already present"
    );
    assert!(
        names.contains(&"age".to_string()),
        "'age' should still appear"
    );
}

/// Enum value completion at a value position.
#[tokio::test]
async fn completion_enum_values() {
    let mut client = TestClient::new();
    client.initialize().await;

    // {"$schema": "...", "mode": null}
    //                           ^ cursor here (value position after colon)
    let content = doc_with_schema(r#""mode": null"#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Position cursor right after ": " before the placeholder value.
    let idx = content.find("\"mode\": ").unwrap() + "\"mode\": ".len();
    let cursor_col = idx as u32;
    let result = client.completion(&uri, 0, cursor_col).await;

    let names = labels(&result);
    assert!(
        names.contains(&r#""dark""#.to_string()),
        "expected '\"dark\"' in enum completions, got: {names:?}"
    );
    assert!(
        names.contains(&r#""light""#.to_string()),
        "expected '\"light\"' in enum completions, got: {names:?}"
    );
    assert!(
        names.contains(&r#""auto""#.to_string()),
        "expected '\"auto\"' in enum completions, got: {names:?}"
    );
}

/// Boolean value completion at a value position.
#[tokio::test]
async fn completion_boolean_values() {
    let mut client = TestClient::new();
    client.initialize().await;

    // {"$schema": "...", "enabled": null}
    //                              ^ cursor here (value position after colon)
    let content = doc_with_schema(r#""enabled": null"#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Position cursor right after ": " before the placeholder value.
    let idx = content.find("\"enabled\": ").unwrap() + "\"enabled\": ".len();
    let cursor_col = idx as u32;
    let result = client.completion(&uri, 0, cursor_col).await;

    let names = labels(&result);
    assert!(
        names.contains(&"true".to_string()),
        "expected 'true' in boolean completions, got: {names:?}"
    );
    assert!(
        names.contains(&"false".to_string()),
        "expected 'false' in boolean completions, got: {names:?}"
    );
}

/// Nested object property completions.
#[tokio::test]
async fn completion_nested_object_properties() {
    let mut client = TestClient::new();
    client.initialize().await;

    // {"$schema": "...", "settings": {}}
    let content = doc_with_schema(r#""settings": {}"#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Cursor inside the nested {} — find position of the inner {
    let inner_brace = content.rfind('{').unwrap();
    let cursor_col = (inner_brace + 1) as u32; // just after {
    let result = client.completion(&uri, 0, cursor_col).await;

    let names = labels(&result);
    assert!(
        names.contains(&"theme".to_string()),
        "expected 'theme' in nested completions, got: {names:?}"
    );
    assert!(
        names.contains(&"verbose".to_string()),
        "expected 'verbose' in nested completions, got: {names:?}"
    );
    // Should NOT contain root-level properties
    assert!(
        !names.contains(&"name".to_string()),
        "'name' should not appear in nested object completions"
    );
}

/// $ref property resolution for completions.
#[tokio::test]
async fn completion_ref_resolved_properties() {
    let mut client = TestClient::new();
    client.initialize().await;

    // {"$schema": "...", "address": {}}
    let content = doc_with_schema(r#""address": {}"#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    // Cursor inside address {}
    let inner_brace = content.rfind('{').unwrap();
    let cursor_col = (inner_brace + 1) as u32;
    let result = client.completion(&uri, 0, cursor_col).await;

    let names = labels(&result);
    assert!(
        names.contains(&"street".to_string()),
        "expected 'street' from $ref'd Address, got: {names:?}"
    );
    assert!(
        names.contains(&"city".to_string()),
        "expected 'city' from $ref'd Address, got: {names:?}"
    );
}

/// No-schema document returns empty/null completions.
#[tokio::test]
async fn completion_no_schema_returns_null() {
    let mut client = TestClient::new();
    client.initialize().await;

    let uri = file_uri("/tmp/no-schema-completion-test.json");
    client.did_open(&uri, "json", 1, r#"{"anything": }"#).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    client
        .recv_notification("textDocument/publishDiagnostics")
        .await;

    let result = client.completion(&uri, 0, 13).await;
    assert!(
        result.is_null(),
        "expected null when no schema is available, got: {result}"
    );
}

/// Malformed document uses stale cache for completions.
#[tokio::test]
async fn completion_malformed_document_uses_stale_cache() {
    let mut client = TestClient::new();
    client.initialize().await;

    // First, open a valid document so the stale cache is populated.
    let valid_content = doc_with_schema(r#""name": "Alice""#);
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &valid_content).await;

    // Now send a malformed edit (simulating typing a new property key).
    // No closing } — truly malformed so parse_jsonc fails.
    let malformed = format!(
        r#"{{"$schema": "{}", "name": "Alice", "#,
        completion_schema_path()
    );
    client.did_change(&uri, 2, &malformed).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Drain diagnostics from the malformed content.
    client
        .recv_notification("textDocument/publishDiagnostics")
        .await;

    // Request completion at the end of the malformed content.
    let cursor_col = malformed.len() as u32;
    let result = client.completion(&uri, 0, cursor_col).await;

    // Should still get completions from stale cache.
    let names = labels(&result);
    assert!(
        !names.is_empty(),
        "expected completions from stale cache, got empty"
    );
    // "name" should be filtered (it's in the stale value).
    assert!(
        !names.contains(&"name".to_string()),
        "'name' should be filtered even from stale cache"
    );
}

/// Completion includes documentation from schema description.
#[tokio::test]
async fn completion_includes_documentation() {
    let mut client = TestClient::new();
    client.initialize().await;

    let content = doc_with_schema("");
    let uri = doc_uri();
    open_and_wait(&mut client, &uri, &content).await;

    let cursor_col = (content.len() - 1) as u32;
    let result = client.completion(&uri, 0, cursor_col).await;

    let items = result["items"].as_array().unwrap();
    let name_item = items.iter().find(|i| i["label"] == "name").unwrap();

    let doc = name_item["documentation"]["value"].as_str().unwrap();
    assert!(
        doc.contains("The user's full name"),
        "expected description in documentation, got: {doc}"
    );
}
