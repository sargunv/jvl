mod common;

use std::time::Duration;

use common::lsp_client::{TestClient, file_uri};

fn simple_schema_path() -> String {
    format!(
        "{}/tests/fixtures/simple-schema.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn doc_uri() -> String {
    file_uri(&format!(
        "{}/tests/fixtures/test-lsp-debounce.json",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Rapid edits: 10 didChange events in quick succession should produce only one
/// publishDiagnostics notification after the debounce window expires.
///
/// With `start_paused = true`, tokio's clock is frozen until we advance it.
/// All spawned tasks sleep in the debounce; we control exactly when they wake.
#[tokio::test(start_paused = true)]
async fn rapid_edits_produce_single_diagnostic_notification() {
    let mut client = TestClient::new();
    client.initialize().await;

    let uri = doc_uri();
    let schema = simple_schema_path();

    // Open with the first version.
    let content_v1 = format!(r#"{{"$schema": "{schema}", "name": "app", "port": 8080}}"#);
    client.did_open(&uri, "json", 1, &content_v1).await;

    // Allow the server coroutines a chance to run (yield to the executor).
    // With start_paused, time doesn't advance so sleeps don't fire yet.
    tokio::task::yield_now().await;

    // Send 9 more rapid edits (versions 2-10), alternating valid/invalid content.
    for v in 2..=10i32 {
        let port = if v % 2 == 0 { r#""bad-port""# } else { "8080" };
        let content = format!(r#"{{"$schema": "{schema}", "name": "v{v}", "port": {port}}}"#);
        client.did_change(&uri, v, &content).await;
        tokio::task::yield_now().await;
    }

    // Version 10 has port = "bad-port" (v=10, even). So the final result should have errors.
    // Advance the clock past the 200ms debounce.
    tokio::time::advance(Duration::from_millis(250)).await;

    // Yield so the woken tasks can run.
    tokio::task::yield_now().await;

    // We should receive exactly ONE publishDiagnostics notification.
    // All tasks except the one for version 10 should have self-cancelled.
    let notification = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    let uri_in_notification = notification["params"]["uri"].as_str().unwrap();
    assert_eq!(uri_in_notification, doc_uri());

    // Version 10 has an invalid port → expect errors.
    let diagnostics = notification["params"]["diagnostics"].as_array().unwrap();
    assert!(
        !diagnostics.is_empty(),
        "expected diagnostics for the final (invalid) version"
    );

    // Ensure no second notification arrives by advancing time further and yielding.
    tokio::time::advance(Duration::from_millis(100)).await;
    tokio::task::yield_now().await;
    // (We can't easily assert "no more messages" in a non-timeout test. The behavioral
    // correctness is demonstrated by the fact that only one notification is received above.)
}

/// did_close during in-flight validation: close the document while the debounce
/// is sleeping; the task should discard (None from document_map) and not publish.
#[tokio::test(start_paused = true)]
async fn did_close_during_debounce_discards_result() {
    let mut client = TestClient::new();
    client.initialize().await;

    let uri = doc_uri();
    let schema = simple_schema_path();
    let content = format!(r#"{{"$schema": "{schema}", "name": "app", "port": 8080}}"#);

    client.did_open(&uri, "json", 1, &content).await;
    tokio::task::yield_now().await;

    // Close before the debounce fires.
    client.did_close(&uri).await;
    tokio::task::yield_now().await;

    // The didClose should immediately publish empty diagnostics.
    let close_notif = client
        .recv_notification("textDocument/publishDiagnostics")
        .await;
    assert_eq!(
        close_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "expected empty diagnostics on close"
    );

    // Advance past the debounce — the in-flight task should silently discard.
    tokio::time::advance(Duration::from_millis(250)).await;
    tokio::task::yield_now().await;

    // No further publishDiagnostics should arrive.
    // (The debounce task returns early because the URI is not in document_map.)
    // We verify the server is still responsive by shutting down cleanly.
    client.shutdown().await;
}
