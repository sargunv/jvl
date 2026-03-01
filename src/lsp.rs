use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use crate::diagnostic::{FileDiagnostic, Severity};
use crate::discover::{self, CompiledFileFilter, CompiledSchemaMappings, Config};
use crate::parse;
use crate::schema::{self, SchemaCache, SchemaSource};
use crate::validate;

/// Compiled jvl.json config with resolved schema mappings.
struct CompiledConfig {
    mappings: CompiledSchemaMappings,
    project_root: PathBuf,
    strict: bool,
    file_filter: CompiledFileFilter,
}

/// Result of resolving config + schema for a single document.
struct ResolvedDocument {
    schema_source: Option<SchemaSource>,
    strict: bool,
    config_log: Option<String>,
}

impl ResolvedDocument {
    fn skip() -> Self {
        Self {
            schema_source: None,
            strict: false,
            config_log: None,
        }
    }

    fn error(msg: String) -> Self {
        Self {
            schema_source: None,
            strict: false,
            config_log: Some(msg),
        }
    }
}

/// LSP server backend.
#[derive(Clone)]
pub struct Backend {
    client: Client,
    /// Open documents: URI → (LSP version, full text content)
    #[allow(clippy::type_complexity)]
    document_map: Arc<Mutex<HashMap<Uri, (i32, Arc<String>)>>>,
    /// jvl.json config cache: config file path → compiled config
    config_cache: Arc<Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>>,
    /// Compiled JSON Schema validator cache (shared with validate_file)
    schema_cache: Arc<SchemaCache>,
    /// Caps concurrent spawn_blocking validations to prevent thread pool pressure
    validation_semaphore: Arc<Semaphore>,
    /// True if the client negotiated UTF-8 position encoding; false = UTF-16 (default).
    utf8_positions: Arc<AtomicBool>,
    /// Schema file paths for which we have registered file watchers.
    watched_schema_paths: Arc<Mutex<HashSet<PathBuf>>>,
    /// Monotonically increasing counter for unique watcher registration IDs.
    next_reg_id: Arc<AtomicU64>,
    /// True if the client supports Markdown in hover content.
    hover_markdown: Arc<AtomicBool>,
    /// Last successfully parsed `serde_json::Value` per document URI.
    /// Used as a fallback when the current document text is malformed.
    last_good_value: Arc<Mutex<HashMap<Uri, Arc<serde_json::Value>>>>,
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
            utf8_positions: Arc::new(AtomicBool::new(false)),
            watched_schema_paths: Arc::new(Mutex::new(HashSet::new())),
            next_reg_id: Arc::new(AtomicU64::new(0)),
            hover_markdown: Arc::new(AtomicBool::new(true)),
            last_good_value: Arc::new(Mutex::new(HashMap::new())),
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

        // Clone self cheaply (all fields are Arc).
        let this = self.clone();

        // Detach the task; it runs until completion unless superseded by `spawn_version` check.
        tokio::spawn(async move {
            this.validate_and_publish(uri, spawn_version).await;
        });
    }

    /// Debounced validation task. Sleeps 200ms, snapshots content+version atomically,
    /// validates in `spawn_blocking`, then publishes diagnostics if the version is current.
    ///
    /// `spawn_version` is the document version at the time this task was spawned.
    /// After sleeping, the task compares the current version to `spawn_version`; if they
    /// differ, a newer edit superseded this task and it self-cancels.
    async fn validate_and_publish(&self, uri: Uri, spawn_version: i32) {
        // 1. Debounce: wait for typing to settle.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // 2. Limit concurrent blocking validations.
        let Ok(_permit) = self.validation_semaphore.acquire().await else {
            return;
        };

        // 3. Snapshot content + version together in ONE lock acquisition AFTER the sleep.
        //    This is critical — capturing content at didChange time can produce stale
        //    content that passes the version guard (the debounce race condition).
        let snapshot = {
            let docs = self.document_map.lock().unwrap_or_else(|e| e.into_inner());
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
        let schema_cache_clone = Arc::clone(&self.schema_cache);
        let config_cache_clone = Arc::clone(&self.config_cache);
        let content_clone = content.clone();
        let file_path_clone = file_path.clone();

        let result = tokio::task::spawn_blocking(move || {
            // Try to parse for the stale value cache (cheap relative to validation).
            let parsed_value = parse::parse_jsonc(&content_clone)
                .ok()
                .map(|p| Arc::new(p.value));

            let resolved = resolve_schema_for_document(&file_path_clone, &config_cache_clone);

            let validate_result = validate::validate_file(
                &path_str,
                &content_clone,
                resolved.schema_source.as_ref(),
                &schema_cache_clone,
                false, // no_cache: always use disk cache in LSP mode
                resolved.strict,
            );

            (validate_result, resolved.config_log, parsed_value)
        })
        .await;

        let ((file_result, warnings, _, _), config_log, parsed_value) = match result {
            Ok(r) => r,
            Err(e) => {
                self.client
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
            self.client.log_message(MessageType::WARNING, msg).await;
        }
        for warning in &warnings {
            self.client
                .log_message(MessageType::WARNING, &warning.message)
                .await;
        }

        // 7. Post-validation version guard: discard if a newer edit arrived during validation
        //    or the document was closed (None from the map → discard, not publish).
        let still_current = {
            let docs = self.document_map.lock().unwrap_or_else(|e| e.into_inner());
            docs.get(&uri).map(|(v, _)| *v == version).unwrap_or(false)
        };

        if !still_current {
            return;
        }

        // 8. Update stale value cache if the parse succeeded.
        if let Some(value) = parsed_value {
            self.last_good_value
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(uri.clone(), value);
        }

        // 9. Convert and publish diagnostics.
        //    Compute line starts once for the full document, then pass to each converter.
        let utf8 = self.utf8_positions.load(Ordering::Relaxed);
        let line_starts = parse::compute_line_starts(&content);
        let diagnostics: Vec<Diagnostic> = file_result
            .errors
            .iter()
            .map(|d| file_diagnostic_to_lsp(d, &content, &line_starts, utf8))
            .collect();

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;

        // Register file watchers for any newly discovered schema files.
        self.update_schema_watchers().await;
    }

    /// Snapshot the current document text for the given URI.
    fn snapshot_document(&self, uri: &Uri) -> Option<Arc<String>> {
        let docs = self.document_map.lock().unwrap_or_else(|e| e.into_inner());
        docs.get(uri).map(|(_, c)| c.clone())
    }

    /// Resolve the schema source for a document URI, compile it, and return
    /// the raw schema `serde_json::Value`. Returns `None` if the URI is not a
    /// file:// URI, no schema is configured, or the schema fails to compile.
    fn resolve_schema_value(
        &self,
        uri: &Uri,
        parsed_value: &serde_json::Value,
    ) -> Option<Arc<serde_json::Value>> {
        let file_path = uri.to_file_path().map(Cow::into_owned)?;
        let resolved = resolve_schema_for_document(&file_path, &self.config_cache);

        let schema_source = resolved
            .schema_source
            .or_else(|| schema::resolve_schema_from_value(parsed_value, &file_path))?;

        match self
            .schema_cache
            .get_or_compile_with_value(&schema_source, false)
        {
            Ok(Some(v)) => Some(v),
            _ => None,
        }
    }

    /// Register file watchers for newly discovered schema file paths.
    ///
    /// Queries the schema cache for all `SchemaSource::File` entries and
    /// registers a watcher for each path not already being watched.
    async fn update_schema_watchers(&self) {
        let new_paths: Vec<PathBuf> = {
            let mut watched = self
                .watched_schema_paths
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.schema_cache
                .cached_file_paths()
                .into_iter()
                .filter(|p| watched.insert(p.clone()))
                .collect()
        };

        if new_paths.is_empty() {
            return;
        }

        let watchers: Vec<FileSystemWatcher> = new_paths
            .iter()
            .map(|p| FileSystemWatcher {
                glob_pattern: GlobPattern::String(escape_glob_metacharacters(
                    &p.display().to_string(),
                )),
                kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
            })
            .collect();

        let reg_id = self.next_reg_id.fetch_add(1, Ordering::Relaxed);
        let registration = Registration {
            id: format!("jvl-schema-watch-{reg_id}"),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions { watchers })
                    .unwrap(),
            ),
        };

        if let Err(e) = self.client.register_capability(vec![registration]).await {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("jvl: failed to register schema file watcher ({e})"),
                )
                .await;
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Negotiate positionEncoding: prefer UTF-8 if the client advertises it.
        let utf8 = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref())
            .and_then(|encs| {
                encs.iter()
                    .find(|e| e.as_str() == PositionEncodingKind::UTF8.as_str())
            })
            .is_some();

        self.utf8_positions.store(utf8, Ordering::Relaxed);

        // Check if client supports Markdown in hover content.
        if let Some(text_doc) = &params.capabilities.text_document
            && let Some(hover_caps) = &text_doc.hover
            && let Some(formats) = &hover_caps.content_format
        {
            self.hover_markdown
                .store(formats.contains(&MarkupKind::Markdown), Ordering::Relaxed);
        }

        let position_encoding = if utf8 {
            PositionEncodingKind::UTF8
        } else {
            PositionEncodingKind::UTF16
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
                hover_provider: Some(HoverProviderCapability::Simple(true)),
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
            docs.insert(uri.clone(), (version, Arc::new(content)));
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
            docs.insert(uri.clone(), (version, Arc::new(content)));
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

        // Remove from stale value cache.
        self.last_good_value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&uri);

        // Clear diagnostics for this document.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // 1. Snapshot document text.
        let Some(content) = self.snapshot_document(&uri) else {
            return Ok(None);
        };

        // 2. Parse the document.
        let parsed = match parse::parse_jsonc(&content) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        // 3. Convert LSP position to byte offset.
        let utf8 = self.utf8_positions.load(Ordering::Relaxed);
        let line_starts = parse::compute_line_starts(&content);
        let Some(byte_offset) = lsp_position_to_byte_offset(&content, &line_starts, position, utf8)
        else {
            return Ok(None);
        };

        // 4. Find JSON pointer at byte offset.
        let Some((pointer, node_range)) = parse::offset_to_pointer(&parsed.ast, byte_offset) else {
            return Ok(None);
        };

        // 5. Resolve schema value for this document.
        let Some(schema_value) = self.resolve_schema_value(&uri, &parsed.value) else {
            return Ok(None);
        };

        // 6. Look up hover content from schema annotations.
        let Some(hover_content) = schema::lookup_hover_content(&schema_value, &pointer) else {
            return Ok(None);
        };

        // 7. Convert node_range back to LSP Range.
        let start_pos = byte_offset_to_lsp_position(&content, &line_starts, node_range.start, utf8);
        let end_pos = byte_offset_to_lsp_position(&content, &line_starts, node_range.end, utf8);

        let kind = if self.hover_markdown.load(Ordering::Relaxed) {
            MarkupKind::Markdown
        } else {
            MarkupKind::PlainText
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind,
                value: hover_content,
            }),
            range: Some(Range::new(start_pos, end_pos)),
        }))
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let changed: Vec<PathBuf> = params
            .changes
            .iter()
            .filter_map(|c| c.uri.to_file_path().map(Cow::into_owned))
            .collect();

        // Evict config cache for jvl.json changes.
        let mut config_changed = false;
        {
            let mut cache = self.config_cache.lock().unwrap_or_else(|e| e.into_inner());
            for path in &changed {
                if path.file_name() == Some(std::ffi::OsStr::new("jvl.json")) {
                    cache.remove(path);
                    config_changed = true;
                }
            }
        }

        // On config change, clear watched schema paths so they are
        // rediscovered during re-validation.
        if config_changed {
            self.watched_schema_paths
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        }

        // Evict schema cache for changed files.
        let mut schema_changed = false;
        for path in &changed {
            if self.schema_cache.evict(&SchemaSource::file(path.clone())) {
                schema_changed = true;
            }
        }

        // Only re-validate if something was actually invalidated.
        if config_changed || schema_changed {
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
}

/// Resolve the schema source for a document by walking up to find jvl.json.
///
/// On config error, returns a `ResolvedDocument` with `config_log` set so the caller can log it.
/// After the first successful load, results are cached by jvl.json path.
fn resolve_schema_for_document(
    path: &Path,
    config_cache: &Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>,
) -> ResolvedDocument {
    // Find the nearest jvl.json by walking up the directory tree.
    let Some(config_path) = discover::find_config_file(path) else {
        return ResolvedDocument::skip();
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
                    return ResolvedDocument::error(format!(
                        "jvl: failed to load {}: {e}",
                        config_path.display()
                    ));
                }
            };

            let raw_root = config_path.parent().unwrap_or(Path::new("."));
            let project_root =
                std::fs::canonicalize(raw_root).unwrap_or_else(|_| raw_root.to_path_buf());

            let mappings = match discover::CompiledSchemaMappings::compile(&config) {
                Ok(m) => m,
                Err(e) => {
                    return ResolvedDocument::error(format!(
                        "jvl: failed to compile schema mappings from {}: {e}",
                        config_path.display()
                    ));
                }
            };

            let file_filter = match CompiledFileFilter::compile(&config) {
                Ok(f) => f,
                Err(e) => {
                    return ResolvedDocument::error(format!(
                        "jvl: failed to compile file patterns from {}: {e}",
                        config_path.display()
                    ));
                }
            };

            let new_compiled = Arc::new(CompiledConfig {
                mappings,
                project_root,
                strict: config.strict,
                file_filter,
            });

            // Use entry().or_insert() to handle concurrent cache misses gracefully
            // (two threads may both compute new_compiled, but only one is stored).
            let mut cache = config_cache.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(cache.entry(config_path).or_insert(new_compiled))
        }
    };

    // Compute the path relative to the project root for glob matching.
    // project_root is already canonicalized at cache-fill time, so we just need
    // to canonicalize the document path to match.
    let canonical_path = std::fs::canonicalize(path);
    let stripped = canonical_path.as_ref().ok().and_then(|abs| {
        abs.strip_prefix(&compiled.project_root)
            .ok()
            .map(|r| r.to_string_lossy().to_string())
    });
    let fallback_warning = if stripped.is_none() {
        Some(format!(
            "jvl: could not relativize path {} against project root {}; \
             glob patterns may not match",
            path.display(),
            compiled.project_root.display(),
        ))
    } else {
        None
    };
    let relative = stripped.unwrap_or_else(|| path.to_string_lossy().to_string());

    // Only validate files that match the config's files patterns.
    if !compiled.file_filter.matches(&relative) {
        return ResolvedDocument::skip();
    }

    ResolvedDocument {
        schema_source: compiled.mappings.resolve(&relative, &compiled.project_root),
        strict: compiled.strict,
        config_log: fallback_warning,
    }
}

/// Convert an LSP `Position` to a byte offset in `source`.
///
/// Returns `None` if the line index is out of range.
fn lsp_position_to_byte_offset(
    source: &str,
    line_starts: &[usize],
    position: Position,
    utf8: bool,
) -> Option<usize> {
    let line_idx = position.line as usize;
    let line_start = *line_starts.get(line_idx)?;
    let line_end = line_starts
        .get(line_idx + 1)
        .copied()
        .unwrap_or(source.len());
    let line_text = &source[line_start..line_end];
    let byte_col = lsp_col_to_byte(line_text, position.character, utf8);
    Some(line_start + byte_col)
}

/// Convert a byte offset in `source` to an LSP `Position`.
///
/// Uses `offset_to_line_col` to find the 1-based line/column, then converts
/// the byte column to the negotiated LSP encoding (UTF-8 or UTF-16).
fn byte_offset_to_lsp_position(
    source: &str,
    line_starts: &[usize],
    offset: usize,
    utf8: bool,
) -> Position {
    let (line_1, col_1) = parse::offset_to_line_col(line_starts, offset);
    let line_idx = line_1.saturating_sub(1);
    let line_start = line_starts.get(line_idx).copied().unwrap_or(source.len());
    let line_end = line_starts
        .get(line_idx + 1)
        .copied()
        .unwrap_or(source.len());
    let line_text = &source[line_start..line_end];
    let byte_col = col_1.saturating_sub(1);
    let lsp_char = byte_col_to_lsp(line_text, byte_col, utf8);
    Position::new(line_idx as u32, lsp_char)
}

/// Convert a byte column offset to an LSP character offset using the negotiated encoding.
fn byte_col_to_lsp(line: &str, byte_col: usize, utf8: bool) -> u32 {
    let safe_col = byte_col.min(line.len());
    if utf8 {
        safe_col as u32
    } else {
        line[..safe_col].encode_utf16().count() as u32
    }
}

/// Convert an LSP character offset back to a byte column offset.
///
/// This is the inverse of `byte_col_to_lsp`. When `utf8` is true, the character
/// offset equals the byte offset. For UTF-16, we scan from the start of the line
/// counting UTF-16 code units until we reach `lsp_char`.
fn lsp_col_to_byte(line: &str, lsp_char: u32, utf8: bool) -> usize {
    if utf8 {
        (lsp_char as usize).min(line.len())
    } else {
        let mut utf16_count: u32 = 0;
        for (byte_idx, ch) in line.char_indices() {
            if utf16_count >= lsp_char {
                return byte_idx;
            }
            utf16_count += ch.len_utf16() as u32;
        }
        line.len()
    }
}

/// Convert a `FileDiagnostic` to an `lsp_types::Diagnostic`.
///
/// `source` is the full document text. `line_starts` is precomputed once per validation
/// cycle and shared across all diagnostics.
fn file_diagnostic_to_lsp(
    diag: &FileDiagnostic,
    source: &str,
    line_starts: &[usize],
    utf8: bool,
) -> Diagnostic {
    let (start, end) = match &diag.location {
        Some(loc) => {
            // loc.line and loc.column are 1-based byte offsets.
            let line_idx = loc.line.saturating_sub(1);
            let line_start = line_starts.get(line_idx).copied().unwrap_or(source.len());
            let line_end = line_starts
                .get(line_idx + 1)
                .copied()
                .unwrap_or(source.len());
            let line_text = source[line_start..line_end]
                .trim_end_matches('\n')
                .trim_end_matches('\r');
            let byte_col = loc.column.saturating_sub(1);
            let start_char = byte_col_to_lsp(line_text, byte_col, utf8);
            let start_pos = Position::new(line_idx as u32, start_char);

            // End position is derived from the span's byte offset + length.
            let end_offset = loc.offset + loc.length;
            let end_pos = byte_offset_to_lsp_position(source, line_starts, end_offset, utf8);

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

/// Escape glob metacharacters in a path string so it is treated as a literal.
fn escape_glob_metacharacters(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Start the LSP server over stdio.
pub async fn run_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
