use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::sync::Semaphore;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use crate::diagnostic::{FileDiagnostic, Severity};
use crate::discover::{self, CompiledSchemaMappings, Config};
use crate::parse;
use crate::schema::{SchemaCache, SchemaSource};
use crate::validate;

/// Negotiated position encoding for LSP diagnostic positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NegotiatedEncoding {
    Utf8,
    Utf16,
}

/// Compiled jvl.json config with resolved schema mappings.
struct CompiledConfig {
    mappings: CompiledSchemaMappings,
    project_root: PathBuf,
}

/// LSP server backend.
pub struct Backend {
    client: Client,
    /// Open documents: URI → (LSP version, full text content)
    document_map: Arc<Mutex<HashMap<Uri, (i32, String)>>>,
    /// jvl.json config cache: config file path → compiled config
    config_cache: Arc<Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>>,
    /// Compiled JSON Schema validator cache (shared with validate_file)
    schema_cache: Arc<SchemaCache>,
    /// Caps concurrent spawn_blocking validations to prevent thread pool pressure
    validation_semaphore: Arc<Semaphore>,
    /// Negotiated position encoding (set during initialize)
    encoding: Arc<RwLock<NegotiatedEncoding>>,
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend").finish()
    }
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            document_map: Arc::new(Mutex::new(HashMap::new())),
            config_cache: Arc::new(Mutex::new(HashMap::new())),
            schema_cache: Arc::new(SchemaCache::new()),
            validation_semaphore: Arc::new(Semaphore::new(8)),
            encoding: Arc::new(RwLock::new(NegotiatedEncoding::Utf16)),
        }
    }

    /// Fire-and-forget task: debounce 200ms, validate, publish diagnostics.
    ///
    /// Captures `spawn_version` at spawn time so that — after the debounce sleep —
    /// the task can detect whether a newer edit arrived and self-cancel. This prevents
    /// multiple concurrent tasks (spawned by rapid edits) from all publishing diagnostics
    /// after the debounce window expires.
    fn spawn_validation(&self, uri: Uri) {
        // Capture the version at spawn time.  If a newer edit arrives before this task
        // wakes up, `current_version` will differ from `spawn_version` and the task discards.
        let spawn_version = {
            let docs = self.document_map.lock().unwrap_or_else(|e| e.into_inner());
            match docs.get(&uri) {
                Some((v, _)) => *v,
                None => return,
            }
        };

        let client = self.client.clone();
        let document_map = Arc::clone(&self.document_map);
        let config_cache = Arc::clone(&self.config_cache);
        let schema_cache = Arc::clone(&self.schema_cache);
        let semaphore = Arc::clone(&self.validation_semaphore);
        let encoding = Arc::clone(&self.encoding);

        // Detach the task; it runs until completion unless superseded by `spawn_version` check.
        tokio::spawn(async move {
            validate_and_publish(
                uri,
                spawn_version,
                client,
                document_map,
                config_cache,
                schema_cache,
                semaphore,
                encoding,
            )
            .await;
        });
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Negotiate positionEncoding: prefer UTF-8 if the client advertises it.
        let enc = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref())
            .and_then(|encs| {
                encs.iter()
                    .find(|e| e.as_str() == PositionEncodingKind::UTF8.as_str())
            })
            .map(|_| NegotiatedEncoding::Utf8)
            .unwrap_or(NegotiatedEncoding::Utf16);

        *self.encoding.write().unwrap_or_else(|e| e.into_inner()) = enc;

        let position_encoding = match enc {
            NegotiatedEncoding::Utf8 => PositionEncodingKind::UTF8,
            NegotiatedEncoding::Utf16 => PositionEncodingKind::UTF16,
        };

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "jvl".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                position_encoding: Some(position_encoding),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Register a file watcher for jvl.json so we invalidate the config
        // cache when the user edits their project config.
        let registration = Registration {
            id: "jvl-config-watch".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![FileSystemWatcher {
                        glob_pattern: GlobPattern::String("**/jvl.json".to_string()),
                        kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
                    }],
                })
                .unwrap(),
            ),
        };

        if let Err(e) = self.client.register_capability(vec![registration]).await {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!(
                        "jvl: failed to register file watcher ({e}); \
                         config changes won't trigger re-validation"
                    ),
                )
                .await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let content = params.text_document.text;

        // Only handle file:// URIs; log and skip others.
        if uri.to_file_path().is_none() {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("jvl: skipping non-file URI: {}", uri.as_str()),
                )
                .await;
            return;
        }

        {
            let mut docs = self.document_map.lock().unwrap_or_else(|e| e.into_inner());
            docs.insert(uri.clone(), (version, content));
        }

        self.spawn_validation(uri);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        // FULL sync: take the first (and only) content change.
        let Some(content) = params.content_changes.into_iter().next().map(|c| c.text) else {
            return;
        };

        {
            let mut docs = self.document_map.lock().unwrap_or_else(|e| e.into_inner());
            docs.insert(uri.clone(), (version, content));
        }

        self.spawn_validation(uri);
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;

        // Remove from document map so any in-flight validation discards its result.
        self.document_map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&uri);

        // Clear diagnostics for this document.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // Evict config cache entries for any changed jvl.json files.
        let changed: Vec<PathBuf> = params
            .changes
            .iter()
            .filter_map(|c| c.uri.to_file_path().map(Cow::into_owned))
            .filter(|p| p.file_name() == Some(std::ffi::OsStr::new("jvl.json")))
            .collect();

        if !changed.is_empty() {
            let mut cache = self.config_cache.lock().unwrap_or_else(|e| e.into_inner());
            for path in &changed {
                cache.remove(path);
            }
        }

        // Re-validate all open documents so diagnostics reflect the updated config.
        let uris: Vec<Uri> = self
            .document_map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();

        for uri in uris {
            self.spawn_validation(uri);
        }
    }
}

/// Debounced validation task. Sleeps 200ms, snapshots content+version atomically,
/// validates in `spawn_blocking`, then publishes diagnostics if the version is current.
///
/// `spawn_version` is the document version at the time this task was spawned.
/// After sleeping, the task compares the current version to `spawn_version`; if they
/// differ, a newer edit superseded this task and it self-cancels.
#[allow(clippy::too_many_arguments)]
async fn validate_and_publish(
    uri: Uri,
    spawn_version: i32,
    client: Client,
    document_map: Arc<Mutex<HashMap<Uri, (i32, String)>>>,
    config_cache: Arc<Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>>,
    schema_cache: Arc<SchemaCache>,
    semaphore: Arc<Semaphore>,
    encoding: Arc<RwLock<NegotiatedEncoding>>,
) {
    // 1. Debounce: wait for typing to settle.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 2. Limit concurrent blocking validations.
    let Ok(_permit) = semaphore.acquire().await else {
        return;
    };

    // 3. Snapshot content + version together in ONE lock acquisition AFTER the sleep.
    //    This is critical — capturing content at didChange time can produce stale
    //    content that passes the version guard (the debounce race condition).
    let snapshot = {
        let docs = document_map.lock().unwrap_or_else(|e| e.into_inner());
        docs.get(&uri).map(|(v, c)| (*v, c.clone()))
    };
    let Some((current_version, content)) = snapshot else {
        return; // Document was closed during the debounce window.
    };

    // 4. Check whether this task is still the right one.
    //    If the current version doesn't match the spawn-time version, a newer
    //    edit arrived while we were sleeping — another task handles that version.
    if current_version != spawn_version {
        return;
    }

    let version = current_version;

    // 5. Resolve file path (non-file URIs were already filtered in did_open).
    let Some(file_path) = uri.to_file_path().map(Cow::into_owned) else {
        return;
    };

    // 6. All blocking I/O (config resolution + validation) in spawn_blocking.
    let path_str = file_path.display().to_string();
    let schema_cache_clone = Arc::clone(&schema_cache);
    let config_cache_clone = Arc::clone(&config_cache);
    let content_clone = content.clone();
    let file_path_clone = file_path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let (schema_source, config_log) =
            resolve_schema_for_document(&file_path_clone, &config_cache_clone);

        let validate_result = validate::validate_file(
            &path_str,
            &content_clone,
            schema_source.as_ref(),
            &schema_cache_clone,
            false, // no_cache: always use disk cache in LSP mode
            false, // strict: silent skip for files without schema
        );

        (validate_result, config_log)
    })
    .await;

    let ((file_result, warnings, _, _), config_log) = match result {
        Ok(r) => r,
        Err(e) => {
            client
                .log_message(
                    MessageType::ERROR,
                    format!("jvl: validation task panicked: {e}"),
                )
                .await;
            return;
        }
    };

    // Log config errors and cache warnings to the editor output panel.
    if let Some(msg) = config_log {
        client.log_message(MessageType::WARNING, msg).await;
    }
    for warning in &warnings {
        client
            .log_message(MessageType::WARNING, &warning.message)
            .await;
    }

    // 7. Post-validation version guard: discard if a newer edit arrived during validation
    //    or the document was closed (None from the map → discard, not publish).
    let still_current = {
        let docs = document_map.lock().unwrap_or_else(|e| e.into_inner());
        docs.get(&uri).map(|(v, _)| *v == version).unwrap_or(false)
    };

    if !still_current {
        return;
    }

    // 8. Convert and publish diagnostics.
    let enc = *encoding.read().unwrap_or_else(|e| e.into_inner());
    let diagnostics: Vec<Diagnostic> = file_result
        .errors
        .iter()
        .map(|d| file_diagnostic_to_lsp(d, &content, enc))
        .collect();

    client.publish_diagnostics(uri, diagnostics, None).await;
}

/// Resolve the schema source for a document by walking up to find jvl.json.
///
/// Returns `(schema_source, optional_error_message)`.
/// On config error, returns `(None, Some(error_message))` so the caller can log it.
/// After the first successful load, results are cached by jvl.json path.
fn resolve_schema_for_document(
    path: &Path,
    config_cache: &Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>,
) -> (Option<SchemaSource>, Option<String>) {
    // Find the nearest jvl.json by walking up the directory tree.
    let Some(config_path) = discover::find_config_file(path) else {
        return (None, None);
    };

    // Check the cache first (fast path — no disk I/O after first load).
    let cached = {
        let cache = config_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.get(&config_path).cloned()
    };

    let compiled = match cached {
        Some(c) => c,
        None => {
            // Cache miss: load and compile the config.
            let config = match Config::load(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    return (
                        None,
                        Some(format!(
                            "jvl: failed to load {}: {e}",
                            config_path.display()
                        )),
                    );
                }
            };

            let project_root = config_path.parent().unwrap_or(Path::new(".")).to_path_buf();

            let mappings = match discover::CompiledSchemaMappings::compile(&config) {
                Ok(m) => m,
                Err(e) => {
                    return (
                        None,
                        Some(format!(
                            "jvl: failed to compile schema mappings from {}: {e}",
                            config_path.display()
                        )),
                    );
                }
            };

            let new_compiled = Arc::new(CompiledConfig {
                mappings,
                project_root,
            });

            // Use entry().or_insert() to handle concurrent cache misses gracefully
            // (two threads may both compute new_compiled, but only one is stored).
            let mut cache = config_cache.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(cache.entry(config_path).or_insert(new_compiled))
        }
    };

    // Compute the path relative to the project root for glob matching.
    let relative = std::fs::canonicalize(path)
        .ok()
        .and_then(|abs| {
            abs.strip_prefix(&compiled.project_root)
                .ok()
                .map(|r| r.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    (
        compiled.mappings.resolve(&relative, &compiled.project_root),
        None,
    )
}

/// Convert a byte column offset to an LSP character offset using the negotiated encoding.
fn byte_col_to_lsp(line: &str, byte_col: usize, enc: NegotiatedEncoding) -> u32 {
    let safe_col = byte_col.min(line.len());
    match enc {
        NegotiatedEncoding::Utf8 => safe_col as u32,
        NegotiatedEncoding::Utf16 => line[..safe_col].encode_utf16().count() as u32,
    }
}

/// Convert a `FileDiagnostic` to an `lsp_types::Diagnostic`.
///
/// `source` is the full document text, used for column encoding conversion.
fn file_diagnostic_to_lsp(
    diag: &FileDiagnostic,
    source: &str,
    enc: NegotiatedEncoding,
) -> Diagnostic {
    let line_starts = parse::compute_line_starts(source);

    let (start, end) = match &diag.location {
        Some(loc) => {
            // loc.line and loc.column are 1-based byte offsets.
            let line_idx = loc.line.saturating_sub(1) as u32;
            let line_text = source.lines().nth(line_idx as usize).unwrap_or("");
            let byte_col = loc.column.saturating_sub(1);
            let start_char = byte_col_to_lsp(line_text, byte_col, enc);
            let start_pos = Position::new(line_idx, start_char);

            // End position is derived from the span's byte offset + length.
            let end_offset = loc.offset + loc.length;
            let (end_line_1based, end_col_1based) =
                parse::offset_to_line_col(&line_starts, end_offset);
            let end_line_idx = end_line_1based.saturating_sub(1) as u32;
            let end_line_text = source.lines().nth(end_line_idx as usize).unwrap_or("");
            let end_byte_col = end_col_1based.saturating_sub(1);
            let end_char = byte_col_to_lsp(end_line_text, end_byte_col, enc);
            let end_pos = Position::new(end_line_idx, end_char);

            (start_pos, end_pos)
        }
        // No source location (schema-fetch/compile errors, or parse errors without span)
        // → point at the start of the file.
        None => (Position::new(0, 0), Position::new(0, 0)),
    };

    let severity = match diag.severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    };

    Diagnostic {
        range: Range::new(start, end),
        severity: Some(severity),
        code: Some(NumberOrString::String(diag.code.clone())),
        source: Some("jvl".to_string()),
        message: diag.message.clone(),
        ..Default::default()
    }
}

/// Start the LSP server over stdio.
pub async fn run_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
