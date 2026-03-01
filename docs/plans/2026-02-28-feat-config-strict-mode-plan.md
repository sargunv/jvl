---
title: "feat: Add per-workspace strict mode to jvl.json"
type: feat
status: completed
date: 2026-02-28
deepened: 2026-02-28
---

# feat: Add per-workspace strict mode to jvl.json

## Enhancement Summary

**Deepened on:** 2026-02-28 **Research agents used:**
pattern-recognition-specialist, performance-oracle, code-simplicity-reviewer,
architecture-strategist, best-practices-researcher, codebase-explorer

### Key Improvements

1. Replace raw tuple return with named `ResolvedDocument` struct for
   `resolve_schema_for_document`
2. Introduce `CompiledFileFilter` wrapper in `discover.rs` (mirrors
   `CompiledSchemaMappings` pattern) instead of exposing private internals
3. Apply `files` glob filtering to ALL LSP validation (not just strict mode) —
   cleaner than a strict-only gate

### New Considerations Discovered

- Editors filter by language ID before sending `didOpen`, so non-JSON files
  rarely reach the LSP — the `files` filtering is defensive but worth doing as
  general LSP behavior
- The CLI already filters files at discovery time, so `files` filtering is only
  needed in the LSP path
- `deny_unknown_fields` + `#[serde(default)]` is the correct serde pattern — no
  compatibility concerns

## Overview

Add a `strict` boolean field (default `false`) to `jvl.json`. When `true`, files
with no resolvable schema produce a `no-schema` error diagnostic instead of
being silently skipped. This applies to both the LSP (editor diagnostics) and
the CLI (`jvl check`), where config `strict` is OR'd with the `--strict` flag.

Refs: [#16](https://github.com/sargunv/jvl/issues/16)

## Problem Statement

The LSP hard-codes `strict: false` (`src/lsp.rs:142`), silently skipping all
files without a schema. There is no way for a workspace to opt into "warn me
about files missing a schema" without modifying source code. The CLI has a
`--strict` flag, but there's no config-file equivalent. Teams that want schema
coverage enforced at the editor level and in CI must remember to pass `--strict`
every time.

## Proposed Solution

**Option 1 from the issue:** Add `strict: bool` (default `false`) to the
`Config` struct. Thread it through the LSP and CLI.

### Key Decisions

| Decision              | Choice                                    | Rationale                                                                                                                                                                                                                                                       |
| --------------------- | ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CLI vs LSP scope      | Both (OR'd)                               | Config `strict: true` applies to both `jvl check` and the LSP. CLI `--strict` flag also enables strict independently. Either being `true` → strict. The flag can never weaken what config specifies — consistent with how `--schema` overrides config mappings. |
| Diagnostic severity   | `Error`                                   | Matches existing `validate_file` behavior (`src/validate.rs:90`). Causes exit code 1 in CLI and red squiggle in editor.                                                                                                                                         |
| File filtering in LSP | All LSP validation gated by `files` globs | The LSP should only validate files matching the config's `files` patterns. This naturally gates strict mode too. Implemented as general LSP behavior via `CompiledFileFilter`.                                                                                  |

## Technical Approach

### A. Add `strict` field to `Config` — `src/discover.rs:52-67`

Add `#[serde(default)] pub strict: bool` to the `Config` struct. Update
`default_config()` at line 157 to include `strict: false`. The `JsonSchema`
derive auto-includes it in `jvl config schema` output.

```rust
// src/discover.rs — Config struct, after the `schemas` field
/// When true, files with no resolvable schema produce an error diagnostic
/// instead of being silently skipped.
#[serde(default)]
pub strict: bool,
```

Also update `default_config()`:

```rust
pub fn default_config() -> Self {
    Config {
        schema_url: None,
        files: default_files(),
        schemas: vec![],
        strict: false,
    }
}
```

### Research Insights

**Serde compatibility:** `deny_unknown_fields` + `#[serde(default)]` is the
standard Rust pattern for backward-compatible config evolution.
`deny_unknown_fields` rejects _unknown_ keys (catches typos);
`#[serde(default)]` provides values for _known but missing_ keys. These are
complementary, not contradictory.

**Precedent:** rust-analyzer, ESLint, and Biome all use per-workspace config to
control diagnostic behavior. jvl's existing `didChangeWatchedFiles`-based config
reload is the clean approach — keeps source of truth in a single
version-controlled file.

### B. Add `CompiledFileFilter` to `discover.rs`

Instead of exposing private
`build_ordered_patterns`/`matches_ordered_patterns`/`PatternEntry` as
`pub(crate)`, introduce a `CompiledFileFilter` wrapper struct. This mirrors the
existing `CompiledSchemaMappings` encapsulation pattern already established in
the codebase.

```rust
// src/discover.rs — new public type
/// Pre-compiled file patterns for efficient per-file matching.
/// Mirrors the encapsulation pattern of `CompiledSchemaMappings`.
pub struct CompiledFileFilter {
    patterns: Vec<PatternEntry>,  // PatternEntry stays private
}

impl CompiledFileFilter {
    /// Compile the `files` patterns from a config.
    pub fn compile(config: &Config) -> Result<Self, ConfigError> {
        let patterns = build_ordered_patterns(&config.files)?;
        Ok(Self { patterns })
    }

    /// Returns true if the relative path matches the file patterns.
    pub fn matches(&self, relative_path: &str) -> bool {
        matches_ordered_patterns(relative_path, &self.patterns)
    }
}
```

Refactor `discover_files` to use `CompiledFileFilter` internally, unifying the
two code paths.

### Research Insights

**Why a wrapper, not `pub(crate)`:** The codebase already demonstrates this
pattern with `CompiledSchemaMappings` (`discover.rs:241-279`) — it wraps
`Vec<CompiledSchemaEntry>` behind a clean `compile()`/`resolve()` API while
keeping `CompiledSchemaEntry` private. `CompiledFileFilter` follows the same
convention. This avoids leaking `PatternEntry` across module boundaries and
keeps glob evaluation logic in `discover.rs`.

**Performance:** Pattern compilation happens once per config load (cache-miss
path). The compiled patterns are stored in `Arc<CompiledConfig>`, so cloning is
a single atomic increment. Matching 2-5 pre-compiled `GlobMatcher`s against a
string is sub-microsecond — negligible compared to JSON parsing (hundreds of
microseconds) and schema validation (milliseconds).

### C. Add `strict` + `file_filter` to `CompiledConfig` — `src/lsp.rs:20-23`

Extend `CompiledConfig` with `strict: bool` and a `CompiledFileFilter`:

```rust
struct CompiledConfig {
    mappings: CompiledSchemaMappings,
    project_root: PathBuf,
    strict: bool,
    file_filter: CompiledFileFilter,
}
```

Update the construction site at `src/lsp.rs:407-410`:

```rust
let file_filter = CompiledFileFilter::compile(&config)?;  // handle error like mappings
let new_compiled = Arc::new(CompiledConfig {
    mappings,
    project_root,
    strict: config.strict,
    file_filter,
});
```

### D. Introduce `ResolvedDocument` struct for return type — `src/lsp.rs:360-433`

Replace the tuple return with a named struct. The current 2-tuple is at the
readability threshold; a bare positional `bool` in a 3-tuple would be fragile.
This follows the codebase's existing preference for named types
(`CompiledConfig`, `CompiledSchemaEntry`, `CompileResult` type alias in
`schema.rs:55-59`).

```rust
/// Result of resolving config + schema for a single document.
struct ResolvedDocument {
    schema_source: Option<SchemaSource>,
    strict: bool,
    config_log: Option<String>,
}
```

Update `resolve_schema_for_document` to return `ResolvedDocument`:

```rust
fn resolve_schema_for_document(
    path: &Path,
    config_cache: &Mutex<HashMap<PathBuf, Arc<CompiledConfig>>>,
) -> ResolvedDocument {
```

Key logic changes:

- **No config found** (line 365-367): Return
  `ResolvedDocument { schema_source: None, strict: false, config_log: None }`
- **Config loaded** (line 419-432): Check
  `compiled.file_filter.matches(&relative)` before resolving. If the file
  doesn't match, return `schema_source: None, strict: false` (skip entirely —
  the LSP should not validate files outside the `files` patterns). If it
  matches, return `compiled.strict` as the strict value.

### Research Insights

**Named struct vs tuple:** The architecture review strongly recommends a named
struct. Future additions (e.g., diagnostic severity overrides) slot in naturally
without a disruptive API change. The existing `validate_file` 4-tuple return
(`src/validate.rs:31-36`) and `CompileResult` type alias in `schema.rs` show the
codebase is already reaching for better names for multi-value returns.

**General file filtering:** By checking `file_filter.matches()` in
`resolve_schema_for_document` and returning early for non-matching files, the
LSP gains general `files` glob filtering for ALL validation — not just strict
mode. This is cleaner than gating only strict mode and addresses the broader
concern that the LSP currently validates any `file://` URI regardless of config
patterns.

**CLI/LSP asymmetry:** In the CLI, `files` filtering happens at discovery time
(`discover_files`). In the LSP, it happens at validation time (in
`resolve_schema_for_document`). This is correct — the CLI walks directories to
find files, while the LSP receives file URIs from the editor. The filtering
serves the same purpose in different architectural contexts.

### E. Use resolved values at the call site — `src/lsp.rs:132-143`

Replace the hardcoded `false` at line 142:

```rust
// Before:
let (schema_source, config_log) =
    resolve_schema_for_document(&file_path_clone, &config_cache_clone);
// ...
false, // strict: silent skip for files without schema

// After:
let resolved = resolve_schema_for_document(&file_path_clone, &config_cache_clone);
// ...
let validate_result = validate::validate_file(
    &path_str,
    &content_clone,
    resolved.schema_source.as_ref(),
    &schema_cache_clone,
    false, // no_cache: always use disk cache in LSP mode
    resolved.strict,
);
(validate_result, resolved.config_log)
```

### F. OR config `strict` with CLI `--strict` — `src/main.rs:602-609`

Compute effective strict once after config loading, then use in the parallel
loop:

```rust
// src/main.rs — after line 403 where config is resolved
let strict = args.strict || config.strict;

// Then in the parallel loop, around line 608:
let (result, file_warnings, cache_outcome, timing) = validate::validate_file(
    path,
    content,
    effective_schema.as_ref(),
    &schema_cache,
    args.no_cache,
    strict,
);
```

The `config` variable is already in scope (line 403). No new plumbing needed.

## Acceptance Criteria

- [x] `jvl.json` accepts a `strict` boolean field (default: `false`) —
      `src/discover.rs`
- [x] When `strict: true`, files without a schema that match the `files` globs
      emit a `no-schema` error diagnostic in the LSP
- [x] When `strict: false` (default), behavior is unchanged — files without
      schema are silently skipped
- [x] CLI `jvl check` respects config `strict` (OR'd with `--strict` flag) —
      `src/main.rs`
- [x] LSP only validates files matching the config `files` patterns (general
      filtering via `CompiledFileFilter`)
- [x] `jvl config schema` includes the `strict` field (automatic via schemars
      derive)
- [x] Existing tests pass
- [x] New LSP test: `strict: true` config + no schema → `no-schema` diagnostic
      emitted
- [x] New LSP test: `strict: true` config + file has schema → no `no-schema`
      diagnostic
- [x] New LSP test: `strict: true` but file doesn't match `files` globs → no
      diagnostic (filtered out)

## Files to Change

| File                       | Change                                                                                                                                                        | Lines                               |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------- |
| `src/discover.rs`          | Add `strict: bool` to `Config`, update `default_config()`, add `CompiledFileFilter` struct, refactor `discover_files` to use it                               | 52-67, 157-163, new struct near 240 |
| `src/lsp.rs`               | Add `strict` + `file_filter` to `CompiledConfig`, add `ResolvedDocument` struct, update `resolve_schema_for_document` return type and logic, use at call site | 20-23, 132-143, 360-433             |
| `src/main.rs`              | OR `args.strict` with `config.strict`                                                                                                                         | ~403, ~608                          |
| `tests/lsp_diagnostics.rs` | Add tests for strict mode and file filtering                                                                                                                  | new tests                           |

## Edge Cases

- **No jvl.json exists** → `strict` defaults to `false`, no file filter → silent
  skip (unchanged behavior)
- **`strict` field absent from jvl.json** → `#[serde(default)]` → `false`
  (backward-compatible)
- **Config changes while editor is open** → file watcher evicts config cache →
  re-validation picks up new `strict` value and new file patterns (existing
  mechanism at `lsp.rs:324-352`)
- **File has inline `$schema`** → schema is resolved in `validate_file` → strict
  path not reached (no `no-schema` diagnostic)
- **File matches a schema mapping** → schema is resolved → strict path not
  reached
- **`--strict` + `--schema` on CLI** → every file has a schema → strict is a
  no-op (correct behavior)
- **File doesn't match `files` globs in LSP** → `resolve_schema_for_document`
  returns early with `strict: false` → no validation at all (silently skipped)
- **Config error loading `files` patterns** → return error in `config_log`, skip
  validation (consistent with existing config error handling at
  `lsp.rs:381-389`)

## Sources

- Issue: [#16](https://github.com/sargunv/jvl/issues/16) — Per-workspace strict
  mode in jvl.json
- `src/validate.rs:24-108` — `validate_file` with existing `strict: bool`
  parameter
- `src/lsp.rs:142` — hardcoded `false` for strict
- `src/discover.rs:49-67` — `Config` struct
- `src/discover.rs:241-279` — `CompiledSchemaMappings` (pattern for
  `CompiledFileFilter`)
- `src/discover.rs:296-333` — `PatternEntry`, `build_ordered_patterns`,
  `matches_ordered_patterns`
- `src/main.rs:109` — CLI `--strict` flag
- `src/schema.rs:55-59` — `CompileResult` type alias (precedent for named return
  types)
- [rust-analyzer configuration](https://rust-analyzer.github.io/book/configuration.html)
  — per-workspace diagnostic control patterns
- [Serde container attributes](https://serde.rs/container-attrs.html) —
  `deny_unknown_fields` + `default` compatibility
