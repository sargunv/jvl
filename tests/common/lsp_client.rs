#![allow(dead_code)]

use std::sync::atomic::{AtomicI64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tower_lsp_server::{LspService, Server};

use jvl::lsp::Backend;

/// In-process LSP test client backed by `tokio::io::duplex`.
///
/// The server runs in a background task on the same tokio runtime. Time-control
/// tests (`#[tokio::test(start_paused = true)]`) work because all async tasks
/// share the same paused clock.
pub struct TestClient {
    write: tokio::io::DuplexStream,
    read: BufReader<tokio::io::DuplexStream>,
    _server: tokio::task::JoinHandle<()>,
    next_id: AtomicI64,
}

impl TestClient {
    pub fn new() -> Self {
        // Two duplex pairs: (client→server) and (server→client).
        let (client_write, server_read) = tokio::io::duplex(65536);
        let (server_write, client_read) = tokio::io::duplex(65536);

        let (service, socket) = LspService::new(Backend::new);
        let server_handle = tokio::spawn(async move {
            Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        Self {
            write: client_write,
            read: BufReader::new(client_read),
            _server: server_handle,
            next_id: AtomicI64::new(1),
        }
    }

    /// Send a raw JSON-RPC message (request or notification) with LSP framing.
    pub async fn send(&mut self, msg: serde_json::Value) {
        let json = serde_json::to_string(&msg).unwrap();
        let header = format!("Content-Length: {}\r\n\r\n", json.len());
        self.write.write_all(header.as_bytes()).await.unwrap();
        self.write.write_all(json.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// Receive the next LSP-framed JSON-RPC message.
    pub async fn recv(&mut self) -> serde_json::Value {
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            self.read.read_line(&mut line).await.unwrap();
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            if let Some(len_str) = line.strip_prefix("Content-Length: ") {
                content_length = len_str.trim().parse().unwrap();
            }
        }
        let mut body = vec![0u8; content_length];
        self.read.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Receive messages, discarding everything except the first message with the
    /// given `method` field. Returns the full message.
    pub async fn recv_notification(&mut self, method: &str) -> serde_json::Value {
        loop {
            let msg = self.recv().await;
            if msg["method"].as_str() == Some(method) {
                return msg;
            }
        }
    }

    /// Send `initialize` request and `initialized` notification; return the
    /// `InitializeResult` capabilities from the response.
    pub async fn initialize(&mut self) -> serde_json::Value {
        self.initialize_with_params(serde_json::json!({})).await
    }

    /// Like `initialize` but allows custom client capabilities.
    pub async fn initialize_with_params(
        &mut self,
        capabilities: serde_json::Value,
    ) -> serde_json::Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "capabilities": capabilities,
                "processId": null,
                "rootUri": null
            }
        }))
        .await;

        // Wait for the response (might receive log messages first, skip them).
        let response = loop {
            let msg = self.recv().await;
            if msg.get("id").is_some() {
                break msg;
            }
        };

        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }))
        .await;

        response["result"].clone()
    }

    /// Send `textDocument/didOpen`.
    pub async fn did_open(&mut self, uri: &str, language_id: &str, version: i32, text: &str) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text
                }
            }
        }))
        .await;
    }

    /// Send `textDocument/didChange` (FULL sync).
    pub async fn did_change(&mut self, uri: &str, version: i32, text: &str) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "version": version
                },
                "contentChanges": [{"text": text}]
            }
        }))
        .await;
    }

    /// Send `textDocument/didClose`.
    pub async fn did_close(&mut self, uri: &str) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": {
                "textDocument": {"uri": uri}
            }
        }))
        .await;
    }

    /// Send `workspace/didChangeWatchedFiles` notification.
    pub async fn did_change_watched_files(&mut self, changes: &[(&str, u32)]) {
        let changes: Vec<serde_json::Value> = changes
            .iter()
            .map(|(uri, kind)| serde_json::json!({"uri": uri, "type": kind}))
            .collect();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": changes }
        }))
        .await;
    }

    /// Send `textDocument/hover` request and return the result.
    pub async fn hover(&mut self, uri: &str, line: u32, character: u32) -> serde_json::Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character }
            }
        }))
        .await;

        // Wait for the response matching our request id.
        // Skip notifications and server-to-client requests (e.g. client/registerCapability).
        let response = loop {
            let msg = self.recv().await;
            if msg.get("id") == Some(&serde_json::json!(id)) && msg.get("method").is_none() {
                break msg;
            }
        };

        response["result"].clone()
    }

    /// Send `textDocument/completion` request and return the result.
    pub async fn completion(&mut self, uri: &str, line: u32, character: u32) -> serde_json::Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character }
            }
        }))
        .await;

        let response = loop {
            let msg = self.recv().await;
            if msg.get("id") == Some(&serde_json::json!(id)) && msg.get("method").is_none() {
                break msg;
            }
        };

        response["result"].clone()
    }

    /// Send `shutdown` request.
    pub async fn shutdown(&mut self) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "shutdown",
            "params": null
        }))
        .await;
        // Drain the response.
        let _ = self.recv().await;
    }
}

/// Convenience: build a `file://` URI from an absolute path string.
#[allow(dead_code)]
pub fn file_uri(path: &str) -> String {
    format!("file://{path}")
}
