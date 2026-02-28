---
title: "feat: Add jvl lsp subcommand for LSP server"
type: feat
status: completed
date: 2026-02-28
origin: docs/brainstorms/2026-02-28-lsp-support-brainstorm.md
---

# feat: Add jvl lsp subcommand for LSP server

## Enhancement Summary

**Deepened on:** 2026-02-28 **Research agents used:** debounce/cancellation
patterns, reqwest+spawn_blocking safety, UTF-16 encoding, LSP testing,
performance oracle, architecture strategist, code simplicity reviewer, race
condition reviewer

### Critical Bugs Found and Fixed

1. **Debounce race condition** — content and version must be snapshotted
   together _after_ the sleep, not at separate times. If content is captured at
   `didChange` and version is checked at publish time, a stale content snapshot
   can pass the version guard. Fixed in Phase 3.

2. **`did_close` + in-flight validation** — version guard must handle `None`
   (document removed from map), not just `version != current`. Fixed in Phase 3.

3. **`did_change` must `tokio::spawn` not `.await`** — awaiting
   `validate_and_publish()` inside the `didChange` handler would serialize all
   keystrokes (200ms sleep × each keystroke). Must be fire-and-forget. Fixed in
   Phase 3.

4. **Static reqwest Client** — `fetch_url()` currently creates a new
   `reqwest::blocking::Client` on every call (spawns an internal OS thread each
   time). Must be hoisted to a `static OnceLock<reqwest::blocking::Client>`.
   Fixed in Phase 1.

### Key Architecture Improvements

5. **`tokio::runtime::Runtime` → `#[tokio::main]`** — The LSP binary entry point
   should use `#[tokio::main]` in a separate `main()` function, not
   `rt.block_on()` in the subcommand handler, to ensure `block_in_place` works
   and multi-thread runtime is active.

6. **Validation semaphore** — Add `tokio::sync::Semaphore(8)` to cap concurrent
   `spawn_blocking` validations and prevent blocking thread pool pressure under
   mass file-open events.

7. **Local schema cache eviction gap** — `SchemaCache` uses `OnceLock` which
   never evicts. Local-file schema changes will NOT be picked up by the current
   design. This is a known Phase 1 limitation; users must restart the server for
   local schema changes. Add `SchemaCache::evict()` in Phase 2.

8. **positionEncoding negotiation** — Implement LSP 3.17 `positionEncoding`
   negotiation. Neovim, Helix, and Zed advertise UTF-8; VS Code uses UTF-16.
   Negotiating UTF-8 avoids the conversion entirely for most modern editors.
   Fall back to UTF-16 for VS Code.

---

## Overview

Add a `jvl lsp` subcommand that starts a Language Server Protocol server over
stdio, providing real-time JSON/JSONC validation diagnostics in any
LSP-compatible editor (VS Code, Neovim, Helix, Zed, etc.). Phase 1 delivers
diagnostics only; completions are deferred (see brainstorm).

## Problem Statement

jvl can validate JSON/JSONC files from the CLI, but there is no editor
integration. Developers using jvl in their projects must run `jvl check`
manually or in CI to see validation errors. An LSP server would surface those
errors inline as the user types, in every editor that supports the standard LSP
protocol.

## Proposed Solution

Implement a `jvl lsp` subcommand using `tower-lsp-server` (the
actively-maintained community fork of tower-lsp). The server:

- Communicates over stdio (editor spawns `jvl lsp` as a child process)
- Maintains an in-memory store of open document contents
- Reuses existing `validate_file()`, `parse.rs`, `schema.rs`, `discover.rs`
  logic unchanged
- Runs blocking validation in `tokio::task::spawn_blocking`
- Registers `json` and `jsonc` language IDs (json5 deferred — see decisions
  below)
- Watches `jvl.json` for changes to invalidate the per-directory config cache

## Technical Approach

### Architecture

```
Editor (LSP client)
    │  stdio (JSON-RPC)
    ▼
jvl lsp (Backend: LanguageServer impl)
    │
    ├── Mutex<HashMap<Url, (u64 version, String content)>>  ← document store
    ├── Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>         ← jvl.json config cache
    ├── Arc<SchemaCache>                                     ← schema compiler cache
    ├── Arc<Semaphore(8)>                                    ← validation concurrency cap
    ├── Arc<AtomicU64>                                       ← global generation counter
    │
    └── tokio::spawn (fire-and-forget per didChange)
            │
            └── sleep(200ms) + snapshot (version, content) atomically
                    │
                    └── spawn_blocking ──► validate_file(path, content, schema, cache)
                                               │
                                               └── convert to Vec<lsp_types::Diagnostic>
                                                        │
                                                        └── check version == current → publish
```

### Key Technical Decisions

**Use `tower-lsp-server = "0.23"` not `tower-lsp = "0.20"`.** The original
`tower-lsp` crate has been stalled for 2+ years. The community fork
(`tower-lsp-server`) is actively maintained (v0.23.0, December 2025), used by
Biome and Oxc, and requires no `#[async_trait]` macro. The API is nearly
identical — same import paths under `tower_lsp::`. (see brainstorm:
docs/brainstorms/2026-02-28-lsp-support-brainstorm.md)

**`validate_file()` already accepts in-memory content.** The function signature
is `validate_file(file_path: &str, source: &str, ...)` — it does NOT read from
disk. `file_path` is used only for the `FileResult.path` display field and as
the base directory for resolving relative `$schema` values. No changes to the
existing API are needed. (Brainstorm Challenge #1 is already solved.)

**`TextDocumentSyncKind::FULL`.** The server receives the complete document text
on every change. Simpler than incremental sync and sufficient for Phase 1.
INCREMENTAL requires applying UTF-16 range-based patches, which is non-trivial
and error-prone. FULL is the right call until profiling shows content transfer
dominates over validation time.

**Per-document config walk-up.** `find_config_file()` is called with each
document's filesystem path. Results are cached in a
`HashMap<PathBuf, Arc<CompiledConfig>>` keyed by the `jvl.json` path. The file
watcher registers `**/jvl.json` to invalidate all affected cache entries on any
config change.

**`#[tokio::main]` for the LSP entry point.** Rather than `rt.block_on()` inside
the subcommand handler, the LSP binary entry uses `#[tokio::main]` in a
dedicated `run_lsp_server()` async function. This ensures the multi-thread
runtime is active, which is required for `tokio::task::block_in_place` (needed
by reqwest blocking inside `spawn_blocking`).

**json5 excluded from Phase 1.** The `jsonc_parser` options in `parse.rs` do not
enable JSON5 syntax (single-quoted strings, hex numbers, etc.). Registering
`json5` would cause false-positive parse errors on valid JSON5 files. Register
only `json` and `jsonc` until JSON5 parse options are added. (SpecFlow finding:
language ID vs. parse option mismatch)

**Debounce + version tracking for rapid edits.** The `didChange` handler fires a
`tokio::spawn` task (NOT awaits — that would serialize keystrokes). The spawned
task sleeps 200ms, then snapshots `(version, content)` together from a single
atomic read of the document map. The snapshot must happen AFTER the sleep, not
at `didChange` time, to avoid the stale-content / valid-version race. A global
`AtomicU64` generation counter enables discarding results from superseded tasks.
(See Race Condition section.)

**200ms is the established industry debounce window.** Deno LSP uses 200ms,
TypeScript LS uses 200ms, independently derived from "150ms between keystrokes
is ~45 WPM."

**UTF-16 column encoding with positionEncoding negotiation.** LSP
`Position.character` is a UTF-16 code unit offset by default. Implement LSP 3.17
`positionEncoding` negotiation: if the client advertises `utf-8` (Neovim, Helix,
Zed), use byte offsets directly. Fall back to UTF-16 for VS Code. The conversion
helper:

```
byte_col_to_utf16(line: &str, byte_col: usize) -> u32
    = line[..byte_col].encode_utf16().count() as u32
```

**Non-`file://` URIs.** Only `file://` URIs are supported. For `untitled:`,
`vscode-vfs://`, or other URI schemes: skip config mapping + inline `$schema`
resolution (relative paths would resolve against `.` — wrong), log once via
`window/logMessage`, publish no diagnostics.

**`SchemaCache` lifetime — known Phase 1 limitation.** URL-based compiled
validators are cached for the server's lifetime (existing behavior). Local-file
schema validators are also cached permanently in Phase 1 via `SchemaCache`'s
`OnceLock`. **This means local schema file changes will not be picked up until
the server restarts.** This is a known limitation. Phase 2 adds
`SchemaCache::evict()` and watches local schema files.

**`jvl.json` parse errors.** If a changed `jvl.json` cannot be parsed, the last
known good config is retained. If `jvl.json` is currently open in the editor, a
parse error diagnostic is published on it. Affected open files keep their last
diagnostics.

**Stale cache warnings go to `window/logMessage`.** The `cache(stale)` warning
from `validate_file()` is an infrastructure concern. Route it to the editor's
output panel, not as a per-document diagnostic.

**`strict = false`, `no_cache = false` always.** In LSP mode: strict is always
false (silent skip for files without schema); the disk cache is always used.

**Shared `reqwest::blocking::Client` via `OnceLock`.** The current `fetch_url()`
creates a new `reqwest::blocking::Client` on every call (spawns an internal OS
thread each time). The client must be hoisted to a
`static OnceLock<reqwest::blocking::Client>` in `schema.rs`. This is a
correctness fix for the LSP context (prevents spawning N OS threads for N
concurrent schema fetches) and applies to the CLI as well.

### File Watcher Registration

Registered in `initialized()` (not `initialize()`) using dynamic capability
registration:

```
**/jvl.json  — WatchKind::all() (create, change, delete)
```

If the client does not support dynamic registration, the failure is logged via
`window/logMessage` and gracefully swallowed. Config watching becomes
unavailable but the server continues to function.

### Schema Resolution Per Document

For each document, the resolution order (matching CLI behavior) is:

1. Look up `find_config_file(document_path)` → locate `jvl.json`
2. If found: load `CompiledConfig` from cache (or parse + compile from disk)
3. Compute relative path: `canonical(doc_path).strip_prefix(project_root)`
4. Call `compiled_mappings.resolve(relative, project_root)` →
   `Option<SchemaSource>`
5. If a schema source was found, pass it to `validate_file()` as
   `Some(&schema_source)`
6. If not found (no config match), pass `None` — `validate_file()` will check
   for inline `$schema`
7. If no schema at all: `validate_file()` returns no errors (strict=false) →
   publish empty diagnostics

### Debounce + Version Tracking (Corrected Pattern)

```rust
// In didChange handler — fire and forget, do NOT await
tokio::spawn(async move {
    // Debounce: wait for typing to settle
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Snapshot content AND version together in one atomic read AFTER sleep.
    // This is critical — do NOT capture content at didChange time.
    let snapshot = {
        let docs = document_map.lock().unwrap();
        docs.get(&uri).map(|(v, c)| (*v, c.clone()))
    };
    let Some((version, content)) = snapshot else {
        return; // document was closed during debounce
    };

    let result = tokio::task::spawn_blocking(move || {
        validate_file(&path, &content, schema, &cache, false, false)
    }).await;

    // Re-read: discard if version advanced OR document was closed
    let still_current = document_map
        .lock()
        .unwrap()
        .get(&uri)
        .map(|(v, _)| *v == version)
        .unwrap_or(false); // None = closed → discard

    if still_current {
        client.publish_diagnostics(uri, diagnostics, None).await;
    }
});
```

Key invariants:

1. Content and version snapshots are from the same lock acquisition, after the
   sleep
2. `None` from the document map (closed document) is treated as "discard", not
   "publish"
3. `did_change` does `tokio::spawn(...)` — never `.await`s
   `validate_and_publish`

### Diagnostic Conversion

```
FileDiagnostic           →    lsp_types::Diagnostic
─────────────────────────────────────────────────────
code (String)            →    code: Some(NumberOrString::String(...))
message (String)         →    message: String
severity (Severity)      →    severity: Some(DiagnosticSeverity::ERROR / WARNING)
location.line (1-based)  →    range.start.line = line - 1  (0-based)
location.column (byte)   →    range.start.character = byte_col_to_lsp(line_text, col - 1)
location.offset+length   →    range.end via offset_to_line_col(line_starts, offset + length)
                         →    source: Some("jvl".to_string())
```

Where `byte_col_to_lsp` dispatches on negotiated encoding:

```rust
fn byte_col_to_lsp(line: &str, byte_col: usize, enc: NegotiatedEncoding) -> u32 {
    match enc {
        NegotiatedEncoding::Utf8  => byte_col as u32,
        NegotiatedEncoding::Utf16 => line[..byte_col].encode_utf16().count() as u32,
    }
}
```

For `SourceLocation = None` (parse errors without spans):
`range = Range { start: (0,0), end: (0,0) }`.

For schema-fetch/compile errors: use `(0,0)` (file start). AST walk for the
`$schema` field span is deferred to Phase 2.

## Implementation Phases

### Phase 1: Dependencies and CLI scaffolding

**Files:** `Cargo.toml`, `src/main.rs`, `src/schema.rs`

**Tasks:**

- Add to `Cargo.toml`:
  ```toml
  tower-lsp-server = "0.23"
  tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-std"] }
  # dashmap = "6"  ← use Mutex<HashMap> instead
  ```
- Add `Lsp` variant to the `Command` enum in `src/main.rs` with no arguments
- In the `Command::Lsp` match arm:
  ```rust
  Command::Lsp => {
      // NOTE: tokio runtime is isolated to this subcommand to avoid
      // making all other subcommands async and to prevent reqwest::blocking
      // from being called outside spawn_blocking on other code paths.
      tokio::runtime::Builder::new_multi_thread()
          .enable_all()
          .thread_name("jvl-lsp")
          .build()
          .expect("failed to build tokio runtime")
          .block_on(jvl::lsp::run_server());
  }
  ```
- Add `pub mod lsp;` to `src/lib.rs`
- **Fix `fetch_url()` in `src/schema.rs`**: hoist `reqwest::blocking::Client` to
  `static OnceLock`:
  ```rust
  static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

  fn get_http_client() -> &'static reqwest::blocking::Client {
      HTTP_CLIENT.get_or_init(|| {
          reqwest::blocking::Client::builder()
              .timeout(HTTP_TIMEOUT)
              .build()
              .expect("failed to build HTTP client")
      })
  }
  ```
  This fix applies to both the CLI and LSP paths. It eliminates the per-call OS
  thread spawning.

**Success criteria:**

- `cargo build` succeeds
- `jvl lsp --help` shows the subcommand
- `cargo clippy` passes with no new warnings

### Phase 2: Backend struct and server skeleton

**Files:** `src/lsp.rs` (new)

**Tasks:**

- Define `Backend` struct:
  ```rust
  #[derive(Debug)]
  struct Backend {
      client: Client,
      document_map: Mutex<HashMap<Url, (u64, String)>>,  // (version, content)
      config_cache: Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>,
      schema_cache: Arc<SchemaCache>,
      validation_semaphore: Arc<Semaphore>,  // cap concurrent spawn_blocking tasks
      generation: Arc<AtomicU64>,
      encoding: Arc<RwLock<NegotiatedEncoding>>,
  }

  #[derive(Debug, Clone, Copy)]
  enum NegotiatedEncoding { Utf8, Utf16 }

  struct CompiledConfig {
      mappings: CompiledSchemaMappings,
      project_root: PathBuf,
  }
  ```

- Implement `LanguageServer` trait stub (all methods return
  `Ok(Default::default())` or `()`)
- `initialize()`:
  - Negotiate `positionEncoding`: check
    `params.capabilities.general.position_encodings`; prefer UTF-8 if advertised
  - Store result in `self.encoding`
  - Return `InitializeResult` with `text_document_sync: FULL`, `server_info`,
    `position_encoding`
- `initialized()`: register `**/jvl.json` file watcher
- `shutdown()`: return `Ok(())`
- `pub async fn run_server()` entry point with stdio transport
- No writes to stdout outside JSON-RPC framing (banner/log messages → stderr or
  `window/logMessage`)

**Success criteria:**

- `jvl lsp` starts and does not crash
- An editor can connect and receives the `InitializeResult`

### Phase 3: Document lifecycle handlers and validation loop

**Files:** `src/lsp.rs`

**Tasks:**

- `did_open`: store `(version=1, content)` in document_map, fire
  `tokio::spawn(validate_and_publish(...))`
- `did_change`: update document_map with `(new_version, new_content)` from
  params, fire `tokio::spawn(validate_and_publish(...))`
- `did_close`: remove from document_map,
  `client.publish_diagnostics(uri, vec![], None).await`
- `validate_and_publish(uri, client, document_map, schema_cache, validation_semaphore, encoding)`:
  ```
  1. sleep(200ms)
  2. Acquire semaphore permit (blocks if 8 validations in flight)
  3. Snapshot (version, content) from document_map — single locked read
     → return if None (document closed)
  4. Resolve schema source for this URI's file path
  5. spawn_blocking: validate_file(path, content, schema, cache, false, false)
  6. On JoinError (panic): log via window/logMessage, return
  7. Re-read document_map: discard if version advanced OR doc closed (None check!)
  8. Convert FileResult.errors → Vec<lsp_types::Diagnostic>
  9. client.publish_diagnostics(uri, diagnostics, None)
  ```
- Generation counter bump: in `did_change`, increment `self.generation` before
  spawning the task
- Abort handles: store `JoinHandle`s in `Mutex<HashMap<Url, AbortHandle>>` to
  cancel the sleeping portion of superseded tasks (prevents accumulation of N
  sleeping tasks under rapid typing)

**Success criteria:**

- Open a JSON file with a `$schema` field → diagnostics appear
- Edit the file → diagnostics update (only most recent version published)
- Close the file → diagnostics cleared
- Rapid edits (10 `didChange` in 100ms) → only last one produces published
  diagnostics

### Phase 4: Schema resolution and config cache

**Files:** `src/lsp.rs`

**Tasks:**

- `resolve_schema_for_document(path: &Path, config_cache: ...) -> Option<SchemaSource>`:
  1. `find_config_file(path)` → `Option<PathBuf>` (jvl.json location)
  2. If found: check config_cache; if miss, parse + compile + insert
  3. Compute `canonical(doc_path).strip_prefix(project_root)` → relative path
  4. `mappings.resolve(relative, project_root)` → `Option<SchemaSource>`
  5. Return `Option<SchemaSource>` (or `None` for inline `$schema` fallback)
- Config load with error handling:
  - `jvl.json` parse error → `window/logMessage` warning, return last-good or
    `None`
  - Config cache insert: use `.entry(path).or_insert_with(|| ...)` pattern for
    atomicity

**Success criteria:**

- File matching a jvl.json mapping is validated against the mapped schema
- File with no config and no `$schema` produces no diagnostics (silent skip)
- `jvl.json` parse error does not crash the server

### Phase 5: Config file watching and cache invalidation

**Files:** `src/lsp.rs`

**Tasks:**

- `did_change_watched_files()`:
  1. For each changed `jvl.json`:
     `config_cache.lock().unwrap().remove(&jvl_path)` — **must complete before
     step 2**
  2. If changed `jvl.json` is in `document_map` (it's open): publish parse error
     diagnostic if config is now invalid
  3. For each open document: fire `tokio::spawn(validate_and_publish(...))`

**Success criteria:**

- Editing `jvl.json` to add/remove a schema mapping causes re-validation of open
  files
- Deleting `jvl.json` causes files that depended on it to get empty diagnostics
  (silent skip)

### Phase 6: Diagnostic conversion with encoding support

**Files:** `src/lsp.rs`

**Tasks:**

- `file_diagnostic_to_lsp(diag: &FileDiagnostic, source: &str, enc: NegotiatedEncoding) -> lsp_types::Diagnostic`:
  - Compute `line_starts` from source via `parse::compute_line_starts`
  - `byte_col_to_lsp(line_text, col - 1, enc)` for start position
  - `parse::offset_to_line_col(line_starts, offset + length)` for end position,
    then `byte_col_to_lsp` for end character
  - `byte_col_to_lsp`: UTF-8 mode → direct cast; UTF-16 mode →
    `line[..byte_col].encode_utf16().count() as u32`
  - Map `Severity::Error → DiagnosticSeverity::ERROR`, `Warning → WARNING`
- Schema-fetch/compile errors:
  `range = Range::new(Position::new(0,0), Position::new(0,0))`
- Route `Warning` items from `validate_file()` to
  `client.log_message(MessageType::WARNING, ...)`

**Success criteria:**

- Diagnostic ranges correctly highlight the failing JSON token in the editor
- Non-ASCII content (e.g., `"name": "André"`) produces correct column positions
  (UTF-16 mode)
- Clients advertising UTF-8 receive byte-column offsets

## System-Wide Impact

### Interaction Graph

`jvl lsp` → `tokio::spawn` → `sleep(200ms)` → semaphore acquire →
`tokio::task::spawn_blocking` → `validate_file()` → `parse_jsonc()` →
`jsonschema::validator.validate()` + `SchemaCache::get_or_compile()` →
`get_http_client().get(url).send()` (for URL schemas, inside spawn_blocking,
safe).

`reqwest::blocking` MUST only ever be called from `spawn_blocking` threads. The
static `OnceLock<Client>` fix ensures only one client is ever constructed.

### Error Propagation

- `spawn_blocking` panic → caught by `JoinError` from `.await` → log via
  `window/logMessage`, publish empty diagnostics (do not crash server)
- `jvl.json` parse error → log + retain last-good config
- Schema fetch failure → `validate_file()` returns `schema(load)` error
  diagnostic at (0,0)
- URI decode failure (non-`file://`) → log + skip validation for that document
- Mutex poison → use `unwrap_or_else(|e| e.into_inner())` to recover

### Race Condition Analysis

| Race                                           | Status                       | Mitigation                                                  |
| ---------------------------------------------- | ---------------------------- | ----------------------------------------------------------- |
| Debounce: stale content with valid version     | **Fixed**                    | Snapshot content+version together AFTER sleep               |
| `did_close` + in-flight validation             | **Fixed**                    | `None` from document_map → discard (not publish)            |
| `did_change` blocks on 200ms sleep             | **Fixed**                    | `tokio::spawn` (fire-and-forget), never `.await`            |
| Config cache concurrent miss                   | Benign dup work              | Use `entry().or_insert_with()` for atomicity                |
| N sleeping tasks accumulate under rapid typing | Low risk                     | `AbortHandle` per document to cancel previous sleeping task |
| Config invalidation + concurrent edit          | Handled by version guard     | Verified safe with correct version guard                    |
| SchemaCache OnceLock concurrent init           | Already correct              | Existing design handles this correctly                      |
| Local schema change not picked up              | **Known Phase 1 limitation** | Restart server; fix in Phase 2 with `SchemaCache::evict()`  |

### State Lifecycle Risks

- Document map grows unboundedly if `didClose` is never called (editor bug).
  Acceptable for Phase 1; add a max-document-count cap in Phase 2.
- Config cache grows to one entry per unique `jvl.json` file discovered. Bounded
  by project structure.
- `SchemaCache` grows with compiled URL validators (existing behavior from CLI).
  Bounded by distinct schemas used. LRU eviction is a Phase 2 concern.

### API Surface Parity

The CLI `jvl check` and `jvl lsp` both call `validate_file()`. Changes to
`validate_file()` (e.g., adding new diagnostic codes) automatically appear in
both. The `static OnceLock<Client>` fix in `schema.rs` also improves the CLI
(avoids per-run client construction overhead).

### Known Phase 1 Limitations

Document these in server startup logs and README:

- Local schema file changes require restarting `jvl lsp` (Phase 2:
  `SchemaCache::evict()`)
- `json5` not registered — json5 files receive no LSP support (Phase 2: json5
  parse options)
- Non-`file://` URIs not validated (untitled documents, remote workspaces)
- jvl.json semantic errors (field typos) not diagnosed — only JSON parse errors
  (Phase 2)

### Integration Test Scenarios

1. **File with inline `$schema` URL** — server fetches schema on first open,
   caches it, publishes diagnostics; second open of same file uses cached schema
   (no network request).
2. **File matching jvl.json mapping** — correct schema applied; editing jvl.json
   to point to a different schema triggers re-validation.
3. **File with no schema** — no diagnostics published, no errors in server logs.
4. **Rapid edits** — 10 `didChange` events fired in 100ms — only the last one
   results in published diagnostics; no stale results published; test with
   `tokio::time::pause()`.
5. **Non-ASCII content** — a file containing `"name": "André"` with a
   `maxLength` violation — diagnostic column points to the correct character
   position (UTF-16 mode and UTF-8 mode).
6. **`did_close` during in-flight validation** — close document while validation
   is in flight; no diagnostics published after close.

## Testing Strategy

### Test Harness: In-Process `tokio::io::duplex`

```rust
// tests/common/lsp_client.rs
struct TestClient {
    write: tokio::io::DuplexStream,
    read: tokio::io::BufReader<tokio::io::DuplexStream>,
    _server: tokio::task::JoinHandle<()>,
}

impl TestClient {
    fn new() -> Self {
        let (client_tx, server_rx) = tokio::io::duplex(65536);
        let (server_tx, client_rx) = tokio::io::duplex(65536);
        let (service, socket) = LspService::new(|c| Backend::new(c));
        let handle = tokio::spawn(async move {
            Server::new(server_rx, server_tx, socket).serve(service).await;
        });
        Self { write: client_tx, read: BufReader::new(client_rx), _server: handle }
    }
}
```

- **Notification receipt**: `recv_notification::<PublishDiagnostics>()` loops
  discarding log messages until `textDocument/publishDiagnostics` arrives
- **Debounce testing**: `#[tokio::test(start_paused = true)]` +
  `tokio::time::advance(Duration::from_millis(200))`
- **Snapshot testing**: `insta::assert_json_snapshot!` with `".uri" => "[uri]"`
  redaction; ranges left unredacted (they are exactly what you're testing)

### Test File Layout

```
tests/
  common/
    lsp_client.rs      ← TestClient struct
  lsp_lifecycle.rs     ← initialize/shutdown smoke tests
  lsp_diagnostics.rs   ← didOpen/didChange/didClose → publishDiagnostics
  lsp_debounce.rs      ← timing tests (start_paused = true)
  lsp_encoding.rs      ← UTF-16/UTF-8 position unit tests
  snapshots/           ← insta snapshot files
```

## Acceptance Criteria

### Functional

- [x] `jvl lsp` subcommand exists and starts the LSP server over stdio
- [x] Editor can connect; `initialize` / `initialized` handshake completes
- [x] `textDocument/didOpen` for a JSON file with `$schema` triggers diagnostics
- [x] `textDocument/didChange` (FULL sync) re-validates and updates diagnostics
- [x] `textDocument/didClose` clears diagnostics for that file
- [x] File with no resolvable schema produces no diagnostics (silent skip)
- [x] Schema-fetch failure produces a diagnostic at (0,0) with `schema(load)`
      code
- [x] Schema validation error produces a diagnostic with correct line/column
      range
- [x] Diagnostic range correctly covers the failing JSON token
- [x] `jvl.json` changes trigger re-validation of all open files
- [x] `jvl.json` parse error does not crash the server; last-good config is
      retained
- [x] Server registers language IDs: `json`, `jsonc` (not `json5`)
- [x] Non-`file://` URIs are handled gracefully (log + skip)

### Non-Functional

- [x] Rapid edits do not publish stale diagnostics (version-tracking with
      correct snapshot timing)
- [x] `did_close` during in-flight validation does not re-publish diagnostics
- [x] Diagnostics use correct position offsets (UTF-16 or UTF-8 per negotiated
      encoding)
- [x] `reqwest::blocking` is called via static OnceLock client (not per-call
      construction)
- [x] `reqwest::blocking` is only called inside `spawn_blocking`
- [x] Server does not crash on any panic inside `spawn_blocking` (panic is
      caught and logged)
- [x] `cargo clippy` passes with no new warnings
- [x] Mutex poison is recovered via `unwrap_or_else(|e| e.into_inner())`

### Quality Gates

- [x] `TestClient` test harness implemented in `tests/common/lsp_client.rs`
- [x] Integration tests covering all 6 scenarios in the Integration Test
      Scenarios section
- [x] Debounce test with `start_paused = true` + `tokio::time::advance()`
- [x] UTF-16 position test with emoji/CJK content
- [x] Diagnostic conversion tested via direct assertions (insta not used)
- [x] `cargo test` passes

## Dependencies and Prerequisites

**New crate dependencies:**

- `tower-lsp-server = "0.23"` (NOT the stalled original `tower-lsp`)
- `tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-std"] }`
- ~~`dashmap = "6"`~~ — Use `Mutex<HashMap>` instead (simplification)

**Dev dependencies:**

- `tokio = { version = "1", features = ["full", "test-util"] }` (test-util for
  time control)

**Existing crates already present:** `serde_json`, `reqwest` (blocking feature),
`rayon`

**Schema.rs fix:** `reqwest::blocking::Client` hoisted to
`static OnceLock<reqwest::blocking::Client>` — applies to CLI and LSP.

**Constraint:** All `reqwest::blocking` calls must remain inside
`spawn_blocking`. The multi-thread tokio runtime (`rt-multi-thread` feature) is
required so that `block_in_place` works if ever needed.

## Risk Analysis

| Risk                                            | Likelihood                 | Mitigation                                                      |
| ----------------------------------------------- | -------------------------- | --------------------------------------------------------------- |
| Stale content+version race under rapid typing   | **High without fix** → Low | Snapshot content+version AFTER sleep in single lock acquisition |
| Stale diagnostics after did_close               | **High without fix** → Low | Check `None` from document_map → discard                        |
| tokio + reqwest blocking deadlock               | Medium                     | All validation in spawn_blocking; static client via OnceLock    |
| Local schema changes not picked up              | Certain in Phase 1         | Document limitation; add `SchemaCache::evict()` in Phase 2      |
| UTF-16 column off-by-one for emoji/CJK          | Medium                     | positionEncoding negotiation; UTF-16 test with emoji fixture    |
| Editor does not support dynamic registration    | Low                        | Graceful fallback — log warning, continue without file watching |
| tower-lsp-server API churn                      | Low                        | Pin to 0.23; community fork is stable                           |
| spawn_blocking thread pool pressure (bulk open) | Low                        | `Semaphore(8)` cap on concurrent validations                    |

## Future Considerations (Phase 2+)

- **`SchemaCache::evict()`** — enable local schema file changes to be picked up
  without restart
- **File watcher for local schemas** — watch `**/*.schema.json` or tracked
  schema paths
- **Schema-driven completions** — requires walking raw `serde_json::Value`
  schema at cursor JSON pointer path (the `jsonschema` crate does not expose
  introspection APIs)
- **Hover documentation** — show schema `description`/`title` on hover
- **`json5` support** — add when `jsonc_parser` options for single-quoted
  strings, hex numbers are enabled
- **Per-workspace `strict` mode setting** — allow opting into no-schema warnings
  via workspace config
- **LRU eviction for URL schema validators** — prevent unbounded memory growth
  in long sessions
- **`jvl.json` semantic validation** — validate config fields against
  `config.schema.json`, publish field-level errors
- **AST walk for `$schema` span** — show schema-fetch errors at the `$schema`
  field, not (0,0)

## Sources & References

### Origin

- **Brainstorm document:**
  [docs/brainstorms/2026-02-28-lsp-support-brainstorm.md](../brainstorms/2026-02-28-lsp-support-brainstorm.md)
  Key decisions carried forward: `jvl lsp` subcommand, diagnostics-first scope,
  reuse of existing lib code, stdio transport, `jvl.json` watching, silent skip
  for no-schema files.

### Internal References

- Schema resolution flow: `src/main.rs` `run_check()` function
- Config discovery: `src/discover.rs` `find_config_file()`,
  `CompiledSchemaMappings::resolve()`
- Validation API: `src/validate.rs` `validate_file()` — already accepts `&str`
  content, no changes needed
- Coordinate utilities: `src/parse.rs` `offset_to_line_col()`,
  `compute_line_starts()`
- Diagnostic types: `src/diagnostic.rs` `FileDiagnostic`, `SourceLocation`
  (1-based line/column)
- HTTP client: `src/schema.rs` `fetch_url()` — needs static OnceLock fix

### External References

- [tower-lsp-server (community fork)](https://github.com/tower-lsp-community/tower-lsp-server)
  — v0.23.0, December 2025; used by Biome, Oxc
- [lsp-types::Diagnostic](https://docs.rs/lsp-types/latest/lsp_types/struct.Diagnostic.html)
- [LSP Specification: textDocument/publishDiagnostics](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_publishDiagnostics)
- [LSP Specification: positionEncoding (3.17)](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#serverCapabilities)
- [LSP Specification: workspace/didChangeWatchedFiles](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#workspace_didChangeWatchedFiles)
- [Deno LSP diagnostics: 200ms debounce rationale](https://github.com/denoland/deno/issues/13022)
- [tokio spawn_blocking docs — abort has no effect once started](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)
- [rust-analyzer: UTF-16 positionEncoding negotiation](https://github.com/rust-lang/rust-analyzer/blob/master/crates/rust-analyzer/src/lsp/capabilities.rs)
- [earthlyls: reference LSP test harness](https://github.com/glehmann/earthlyls/tree/main/tests)
- [The bottom emoji breaks rust-analyzer (fasterthanli.me)](https://fasterthanli.me/articles/the-bottom-emoji-breaks-rust-analyzer)
