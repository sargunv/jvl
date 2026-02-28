---
date: 2026-02-28
topic: lsp-support
---

# LSP Support for jvl

## What We're Building

A Language Server Protocol implementation for jvl, exposed as a `jvl lsp`
subcommand. The server communicates over stdio and provides real-time JSON/JSONC
validation diagnostics in any LSP-compatible editor (VS Code, Neovim, Helix,
Zed, etc.).

**Phase 1 scope:** Diagnostics only — surface jvl's existing validation errors
as LSP diagnostics as the user types.

**Phase 2 (future):** Schema-driven completions and hover documentation.
Deferred until we decide on a schema traversal approach (the `jsonschema` crate
doesn't expose introspection APIs).

## Why This Approach

**Diagnostics-first** because the `jsonschema` crate is a validator, not a
schema introspector. Completions require walking the raw schema JSON at a cursor
path — achievable but out of scope for the initial implementation. Starting with
diagnostics delivers immediate value without incurring that complexity.

**`jvl lsp` subcommand** (not a separate binary) keeps distribution simple — one
binary to install.

**stdio transport** is the standard LSP approach: editors spawn jvl as a child
process and communicate over stdin/stdout. Works universally across all LSP
clients.

**Reuse existing lib code** — `validate_file()`, `parse.rs`, `schema.rs`,
`discover.rs` are all reusable. The LSP layer is purely a new way to invoke
existing functionality.

## Key Decisions

- **Framework**: `tower-lsp` crate for the LSP protocol layer (JSON-RPC routing,
  capability negotiation, lifecycle). Async architecture.
- **Transport**: stdio only (standard, universal). No TCP option initially.
- **Schema resolution**: Reuse all three existing mechanisms unchanged: inline
  `$schema` field, `jvl.json` config mapping. No `--schema` flag equivalent for
  LSP — files without a resolvable schema are silently skipped (no diagnostics
  published).
- **Document store**: Maintain an in-memory store of open document contents
  (received via `textDocument/didOpen` / `textDocument/didChange`). Validate the
  in-memory content, not the on-disk file.
- **Blocking I/O**: Existing validation uses blocking I/O (reqwest blocking,
  disk reads). Run validation in `tokio::task::spawn_blocking` to avoid blocking
  the async runtime.
- **Coordinate conversion**: jvl uses 1-based line/column; LSP uses 0-based.
  Convert at the LSP boundary.
- **Diagnostic mapping**: Map existing `FileDiagnostic` → LSP `Diagnostic`.
  Fields map cleanly: `code`, `message`, `severity`, `location` → `range`.

## Key Challenges

1. **`validate_file()` reads from disk** — need a variant that accepts in-memory
   content. Options: add a `validate_content(path, content, ...)` function to
   `validate.rs`, or refactor to separate "read file" from "validate string".

2. **URI → path conversion** — LSP uses `file://` URIs for document identifiers.
   Need to convert to filesystem paths for schema config resolution (`jvl.json`
   discovery walks up from the file path, which works once the URI is decoded).

## Resolved Questions

- **No-schema behavior**: Silent skip. Files without a resolvable schema produce
  no diagnostics. Avoids noise for projects not fully configured with jvl.
- **Config watching**: Yes — use LSP's `workspace/didChangeWatchedFiles` to
  monitor `jvl.json`. Re-validate all open files when it changes.
- **Schema fetch/compile errors**: Surface as LSP diagnostics on the document
  (e.g., at the `$schema` field location if present, otherwise at the file
  start).
- **Language IDs**: Register for `json`, `jsonc`, and `json5`.

## Next Steps

→ `/workflows:plan` for implementation details
