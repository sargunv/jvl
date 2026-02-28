mod common;

use common::lsp_client::TestClient;

/// Verifies the server completes the initialize / initialized / shutdown handshake.
#[tokio::test]
async fn initialize_and_shutdown() {
    let mut client = TestClient::new();
    let result = client.initialize().await;

    // Server advertises FULL sync and reports its name.
    assert_eq!(
        result["capabilities"]["textDocumentSync"],
        serde_json::json!(1) // TextDocumentSyncKind::FULL = 1
    );
    assert_eq!(result["serverInfo"]["name"], "jvl");

    client.shutdown().await;
}

/// Verifies the server negotiates UTF-8 when the client advertises it.
#[tokio::test]
async fn negotiate_utf8_encoding() {
    let mut client = TestClient::new();
    let result = client
        .initialize_with_params(serde_json::json!({
            "general": {
                "positionEncodings": ["utf-8"]
            }
        }))
        .await;

    assert_eq!(result["capabilities"]["positionEncoding"], "utf-8");
}

/// Verifies the server falls back to UTF-16 when the client doesn't advertise UTF-8.
#[tokio::test]
async fn negotiate_utf16_encoding_fallback() {
    let mut client = TestClient::new();
    let result = client.initialize().await;

    // Default: UTF-16.
    assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
}
