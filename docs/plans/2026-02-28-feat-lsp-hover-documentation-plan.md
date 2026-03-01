---
title: "feat: LSP hover documentation from schema description/title"
type: feat
status: completed
date: 2026-02-28
deepened: 2026-02-28
---

# feat: LSP hover documentation from schema description/title

## Enhancement Summary

**Deepened on:** 2026-02-28 **Agents used:** architecture-strategist,
performance-oracle, code-simplicity-reviewer, pattern-recognition-specialist,
security-sentinel, codebase explorer, Context7 (tower-lsp, lsp-types)

### Key Improvements

1. Simplified v1 scope: defer `allOf`/`anyOf`/`oneOf` and `additionalProperties`
   to reduce complexity ~25-30%
2. Performance: run hover inline (no `spawn_blocking`), use `Arc<String>` for
   document text
3. Simplified types: use tuples instead of `HitResult` struct, return formatted
   `Option<String>` instead of `SchemaAnnotation`
4. Security: explicit guard against non-fragment `$ref` values, input clamping
   for positions
5. Use `HashSet` visited-set for `$ref` cycle detection instead of arbitrary
   depth counter

### Reviewer Findings Incorporated

- **Architecture:** Hover should bypass `validate.rs` (direct `lsp` -> `parse` +
  `schema`). `lookup_schema_annotation` is fine in `schema.rs` for now; extract
  to dedicated module when completions (#13) lands.
- **Performance:** Re-parsing on every hover is acceptable for v1 (<10ms for
  typical files). Cache parsed AST later if profiling shows need.
- **Simplicity:** Cut from 12 to 6 integration tests. Defer composition
  keywords. Use simple return types.
- **Security:** All LOW risk. Guard non-fragment `$ref`, clamp out-of-bounds
  positions, truncate annotations.

---

## Overview

Add `textDocument/hover` support to the jvl LSP server so that hovering over a
JSON key or value in an editor shows the schema's `title` and `description`
annotations. This is a high-visibility feature that surfaces schema
documentation inline.

Related: [#14](https://github.com/sargunv/jvl/issues/14),
[#13](https://github.com/sargunv/jvl/issues/13) (completions shares
position-to-pointer primitive)

## Problem Statement / Motivation

Currently, hovering over JSON content in an LSP-enabled editor shows nothing.
Users must consult external schema documentation to understand what a field
means, its purpose, or valid values. Every major JSON LSP (VS Code's built-in,
yaml-language-server) supports hover, and its absence makes jvl feel incomplete.

## Proposed Solution

Implement Option 1 from the issue: walk the raw schema `serde_json::Value` at
the cursor's JSON pointer path to extract `title`/`description` annotations.

### Implementation Steps

#### 1. Store raw schema JSON in SchemaCache (`src/schema.rs`)

Extend `SlotResult` to retain the raw `serde_json::Value` alongside the compiled
`Validator`:

```rust
// src/schema.rs — SlotResult (private struct)
struct SlotResult {
    validator: Result<Arc<jsonschema::Validator>, SchemaError>,
    schema_value: Option<Arc<serde_json::Value>>,  // NEW
    warnings: Vec<Warning>,
    cache_outcome: Option<CacheOutcome>,
}
```

In `get_or_compile()` (line ~559), clone the parsed `schema_value` into an `Arc`
before passing it to `jsonschema::options().build()`. Add a public accessor that
keeps `SlotResult` internals encapsulated:

```rust
// src/schema.rs — SchemaCache
pub fn get_schema_value(&self, source: &SchemaSource) -> Option<Arc<serde_json::Value>>
```

<details>
<summary>Research Insights</summary>

**Architecture (architecture-strategist):** Storing `Arc<serde_json::Value>` in
`SlotResult` is architecturally sound. The raw JSON is already parsed inside
`get_or_compile()` -- retaining it is zero additional parsing cost. The `Arc`
wrapper means hover reads are lock-free after initial compilation.

**Memory (performance-oracle):** For a large schema like Kubernetes API (~4MB
JSON), expect ~16MB of heap for the `serde_json::Value` tree. This is acceptable
for a desktop LSP server. Typical schemas are much smaller. If memory becomes a
concern later, migrate to a pre-extracted annotations map.

**Security (security-sentinel):** `SlotResult` is private to `schema.rs`. Adding
a field has zero external impact. The `CompileResult` type alias is not changed.

</details>

#### 2. Add LSP position to byte offset conversion (`src/lsp.rs`)

Write the inverse of `byte_col_to_lsp` (line 568):

```rust
// src/lsp.rs — module-private, same as byte_col_to_lsp
fn lsp_col_to_byte(line_text: &str, lsp_char: u32, utf8: bool) -> usize
```

Given a line's text and the LSP character offset (in UTF-8 or UTF-16 depending
on negotiated encoding), return the byte offset within that line. Combined with
`compute_line_starts`, this converts an LSP `Position` to a document byte
offset.

<details>
<summary>Research Insights</summary>

**Pattern consistency (pattern-recognition-specialist):** `byte_col_to_lsp` is
module-private (no `pub`). The new `lsp_col_to_byte` should also remain
module-private. The naming convention mirrors the existing function correctly.

**UTF-16 correctness (performance-oracle):** Handle surrogate pairs correctly.
An emoji (U+1F600) is 4 bytes in UTF-8 but 2 code units in UTF-16. Count code
units, not characters. Cost is O(line_length) per conversion but only one per
hover request -- negligible.

**Input safety (security-sentinel):** Apply existing clamping patterns from
`byte_col_to_lsp`:

- Use `line_starts.get(line_idx)` (returns `Option`) rather than indexing
- Clamp byte offsets to `source.len()`
- Return `Ok(None)` from the hover handler for any out-of-bounds position

</details>

#### 3. Add byte-offset-to-JSON-pointer in parse module (`src/parse.rs`)

Add a function that walks the `jsonc_parser` AST to find which node contains a
given byte offset and builds the JSON pointer path:

```rust
// src/parse.rs — return tuple, not a named struct
pub fn offset_to_pointer(ast: &AstValue, offset: usize) -> Option<(Vec<String>, Range<usize>)>
//                                                                  ^pointer     ^node_range
```

**Algorithm:**

1. Check if `offset` falls within the AST root's range (via `Ranged` trait). If
   not, return `None`.
2. For `AstValue::Object`: iterate `.properties`, check if offset is within any
   `ObjectProp`'s key (via `ObjectPropName`) or value range. If on the key, push
   the key name (`name.as_str()`) and return with the key's range. If on the
   value, push the key name and recurse into the value.
3. For `AstValue::Array`: iterate `.elements`, find which element's range
   contains the offset, push the index as a string, recurse.
4. For scalar values (`StringLit`, `NumberLit`, `BooleanLit`, `NullKeyword`):
   the offset is within this leaf node, return current path with the node's
   range.
5. If offset falls on structural tokens (braces, brackets, commas, colons) or
   whitespace between nodes, return `None`.

**Key vs value hover:** When the cursor is on a property key, the pointer path
should point to that property (same as hovering its value). The range should
reflect whichever token (key or value) the cursor is actually on.

**Unit tests:** Add targeted `#[cfg(test)]` tests directly in `parse.rs` (the
module already has a `mod tests` section at line 226). Test cases for
`offset_to_pointer`:

- Offset on a property key
- Offset on a property value
- Offset on a nested path
- Offset on whitespace/structural tokens (returns `None`)
- Offset on array elements

<details>
<summary>Research Insights</summary>

**Simplicity (code-simplicity-reviewer):** Use a tuple
`(Vec<String>, Range<usize>)` instead of a `HitResult` struct. The project's
`parse.rs` returns standard types (`(usize, usize)`, `Option<Range<usize>>`,
`Option<&str>`) -- a named struct for a two-field return consumed in one place
is unnecessary. If completions (#13) needs a different shape, refactor then.

**Pattern consistency (pattern-recognition-specialist):** The name
`offset_to_pointer` fits the `offset_to_*` naming convention established by
`offset_to_line_col`. Do NOT name it `HitResult` -- that's too generic for this
module's conventions.

**Architecture (architecture-strategist):** This is the correct home. `parse.rs`
already owns `resolve_pointer` (pointer -> byte range) and
`resolve_pointer_key`. `offset_to_pointer` is the inverse. These share the same
AST-walking concern. Document boundary behavior: `None` means "no meaningful
JSON node at this position."

**Performance (performance-oracle):** For the AST walk, properties are stored in
document order with monotonically increasing byte ranges. Binary search is
possible but linear scan is fine for v1 -- typical JSON objects have <50
properties per level, well under 1us. Optimize to binary search later if
profiling shows need.

**Generality note (code-simplicity-reviewer):** Do NOT "design generically" for
completions. Design for hover. When completions needs it, refactor if needed.

</details>

#### 4. Add schema annotation lookup (`src/schema.rs`)

Add a function that walks a raw schema `serde_json::Value` following a JSON
pointer path and returns formatted hover content directly:

```rust
// src/schema.rs — returns formatted markdown string, not an intermediate struct
pub fn lookup_hover_content(
    schema: &serde_json::Value,
    pointer: &[String],
) -> Option<String>
```

**Schema traversal rules (v1 scope):**

- For each segment in the pointer, descend via `properties.<segment>` for object
  keys.
- For numeric segments in arrays, descend via `items` (single schema) or
  `prefixItems[i]` (positional).
- Follow `$ref` strings by resolving **fragment-only** references within the
  schema document (e.g., `#/$defs/Foo` -> look up `$defs.Foo`). **Reject any
  `$ref` not starting with `#`** -- return `None` for that branch.
- Use a `HashSet<&str>` of visited `$ref` targets for cycle detection (not a
  depth counter).

**v1 explicitly defers:**

- `allOf`/`anyOf`/`oneOf` composition (best-effort heuristics show wrong
  results; better to show nothing)
- `additionalProperties` fallback (can show misleading annotations)
- `patternProperties`, `if/then/else`, `$dynamicRef`

At the terminal path segment, extract `title` and `description` from the
resolved subschema and format as markdown:

- Both present: `"**{title}**\n\n{description}"`
- Title only: `"**{title}**"`
- Description only: `"{description}"`
- Neither: return `None`

Truncate `title` and `description` to 10,000 characters each to prevent
excessively large hover responses from malicious schemas.

**Unit tests:** Add `#[cfg(test)]` tests directly in `schema.rs`:

- Simple `properties` lookup
- Nested path through multiple `properties` levels
- `$ref` to `$defs` resolution
- `$ref` cycle returns `None`
- Non-fragment `$ref` returns `None`
- Missing annotation returns `None`
- `items` / `prefixItems` for arrays

<details>
<summary>Research Insights</summary>

**Simplicity (code-simplicity-reviewer):** Return formatted `Option<String>`
directly instead of a `SchemaAnnotation` struct. The function already knows the
output format. This eliminates one type, one formatting function, and ~10 lines.
The struct exists solely to shuttle two values across one function boundary
consumed in one place.

**Simplicity (code-simplicity-reviewer):** Deferring `allOf`/`anyOf`/`oneOf`
eliminates ~25-35 lines of branching logic and the combinatorial test space. The
"first subschema with annotations" heuristic would sometimes show the wrong
thing, confusing users more than showing nothing. Real-world schemas that rely
on composition keywords for annotations are uncommon -- most have annotations
directly at the `properties` level. Add when a user reports it missing.

**Security (security-sentinel):** MUST validate that `$ref` values start with
`#` before following them. The `lookup_hover_content` function walks raw JSON
and must never issue network requests or read files. Explicit guard:

```rust
if !ref_str.starts_with('#') { return None; }
```

Truncate annotations to prevent large hover responses from malicious schemas.

**Cycle detection (code-simplicity-reviewer):** A `HashSet<&str>` of visited
`$ref` targets is simpler and more correct than a depth counter. It prevents
true cycles without an arbitrary constant. A depth limit of 20 solves a problem
that doesn't exist in practice (non-cyclic nesting rarely exceeds 5-6 levels).

**Architecture (architecture-strategist):** `schema.rs` is acceptable for now
since the function is self-contained and has data affinity with `SchemaCache`.
When completions (#13) or additional annotation types are added, extract to a
dedicated `schema_walk.rs` module. The function signature has no dependency on
`SchemaCache` internals, so extraction is trivial.

**Architecture (architecture-strategist):** For hover, showing only the leaf
annotation is correct (same as VS Code's JSON language features). Don't
aggregate annotations from parent schemas.

</details>

#### 5. Implement hover handler (`src/lsp.rs`)

Advertise the capability and implement the handler:

```rust
// src/lsp.rs — in initialize(), add to ServerCapabilities (line ~312):
hover_provider: Some(HoverProviderCapability::Simple(true)),
```

```rust
// src/lsp.rs — implement hover on Backend
// Trait signature: async fn hover(&self, params: HoverParams) -> Result<Option<Hover>>
async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
    // 1. Extract URI and position from params.text_document_position_params
    // 2. Snapshot document text from document_map (lock, clone, release)
    // 3. Parse with parse_jsonc (returns ParsedFile with ast + line_starts)
    // 4. Convert LSP position to byte offset:
    //    a. line_starts[position.line] gives line start byte offset
    //    b. lsp_col_to_byte(line_text, position.character, utf8) gives byte offset within line
    // 5. offset_to_pointer(ast, byte_offset) -> Option<(pointer, node_range)>
    // 6. Resolve schema source via resolve_schema_for_document (reuse validation path)
    // 7. schema_cache.get_schema_value(source) -> Option<Arc<Value>>
    // 8. lookup_hover_content(schema_value, &pointer) -> Option<String>
    // 9. Convert node_range back to LSP Range using offset_to_line_col + byte_col_to_lsp
    // 10. Return Ok(Some(Hover { contents: HoverContents::Markup(MarkupContent {
    //         kind: MarkupKind::Markdown, value: content }), range: Some(lsp_range) }))
    //     Or Ok(None) at any failure point
}
```

**Response type:** Use
`HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value })` --
this is the modern LSP approach (vs deprecated `MarkedString`).

**Error handling:** Return `Ok(None)` for any failure (no schema, parse error,
pointer not found, no annotations). Never return `Err(...)` to the client for
hover -- this matches LSP conventions.

**No `spawn_blocking`:** Run hover computation inline on the tokio runtime. The
workload (parse JSONC + walk AST + walk schema JSON) completes in <10ms for
typical files. `spawn_blocking` adds scheduling overhead and complexity for no
benefit. Validation uses `spawn_blocking` because
`jsonschema::Validator::iter_errors()` is CPU-intensive -- hover is not.

**Mutex safety:** Snapshot document text and release `document_map` lock BEFORE
calling `resolve_schema_for_document` or accessing `SchemaCache`. This maintains
the same lock ordering as the existing validation path and prevents any deadlock
risk.

<details>
<summary>Research Insights</summary>

**Architecture (architecture-strategist):** Hover introduces a new direct
dependency `lsp` -> `parse` + `schema`, bypassing `validate.rs`. This is correct
-- hover does not validate, so it should not go through `validate.rs`. The hover
handler is pure orchestration, mirroring how `validate_and_publish` calls into
parse, schema, and validate modules.

**Performance (architecture-strategist + performance-oracle):** Do NOT use
`spawn_blocking` for hover. Validation uses it because
`jsonschema::Validator::iter_errors()` is CPU-intensive and blocking. Hover's
workload is: parse (~1-3ms), AST walk (~0.01ms), schema walk (~0.1-1ms). For a
latency-sensitive operation where work is trivially fast, `spawn_blocking` adds
overhead without benefit.

**Concurrency (security-sentinel):** No deadlock risk. The hover handler
acquires `document_map` for a quick snapshot and releases immediately.
`resolve_schema_for_document` acquires `config_cache` separately. The existing
code never holds two mutexes simultaneously, and hover follows the same pattern.

**LSP types (codebase explorer):** Confirmed exact types from
`tower_lsp_server::ls_types`:

- `HoverParams` has `text_document_position_params` (flattened) containing
  `text_document.uri` and `position`
- `Hover { contents: HoverContents, range: Option<Range> }`
- `HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: String })`
- `HoverProviderCapability::Simple(true)` for capability advertisement

**Staleness (performance-oracle):** If the document changes mid-hover, the
result may be slightly stale. This is acceptable -- the user will move the
cursor and re-hover. No debounce needed for hover (unlike validation).

</details>

#### 6. Add test infrastructure (`tests/common/lsp_client.rs`)

Add a request-response helper to `TestClient` following the existing request
pattern (same as `initialize`):

```rust
// tests/common/lsp_client.rs
pub async fn hover(&mut self, uri: &str, line: u32, character: u32) -> serde_json::Value
```

Implementation:

1. Allocate an ID via `self.next_id.fetch_add(1, Ordering::Relaxed)`
2. Send a `textDocument/hover` JSON-RPC request with the URI and
   `Position { line, character }`
3. Loop on `self.recv()` until a response with matching `id` is received
4. Return `response["result"].clone()` (raw JSON -- tests can inspect freely)

<details>
<summary>Research Insights</summary>

**Pattern consistency (pattern-recognition-specialist + codebase explorer):**
Hover is a **request** (not notification), so it must follow the `initialize`
pattern: send with an `id`, loop `recv()` until matching `id` response. The
existing `did_open`/`did_change`/`did_close` are notifications (no response).
The return type should be `serde_json::Value` for flexible test assertions,
consistent with how `initialize` returns raw JSON.

</details>

#### 7. Add integration tests (`tests/lsp_hover.rs`)

Core test cases (v1 -- 6 tests covering actual functionality):

- [x] **Simple property hover:** Object with `properties` schema, hover on key
      shows `title`/`description`
- [x] **Value hover:** Hovering on a value shows same annotations as its key
- [x] **Nested property:** Hover on a deeply nested key resolves through
      multiple `properties` levels
- [x] **`$ref` resolution:** Schema with `$ref` to `$defs`, annotations from
      referenced definition shown
- [x] **No annotation:** Hover on a field with no `title`/`description` returns
      `null`
- [x] **No schema file:** Hover returns `null` when no schema is associated

Test structure follows existing conventions:

- Start with `mod common;`, import `common::lsp_client::{TestClient, file_uri}`
- Use `#[tokio::test]` attribute
- Call `TestClient::new()`, then `client.initialize().await`
- Use `tempfile::tempdir()` for filesystem fixtures
- Wait past debounce window
  (`tokio::time::sleep(Duration::from_millis(300)).await`) after `did_open`
  before sending hover

Fixture files needed in `tests/fixtures/`:

- A JSON schema with `title`/`description` on properties, including a `$ref` to
  `$defs`
- A JSON document referencing that schema via `$schema`

<details>
<summary>Research Insights</summary>

**Simplicity (code-simplicity-reviewer):** Cut from 12 to 6 tests. The removed
tests cover edge cases that are correct by construction:

- "Whitespace/structural tokens" -- `offset_to_pointer` returns `None` by
  definition
- "JSONC comments" -- comments are not AST nodes, so not findable
- "`$schema` field hover" -- this is the "no annotation" case
- "jvl.json mapping" -- schema resolution is already tested in diagnostics tests
- "UTF-16 encoding" -- already covered by `tests/lsp_encoding.rs`; one hover
  test with multi-byte chars can be added later

The existing LSP test files average 3-5 meaningful test cases each. 6 tests is
proportionate.

**Unit tests vs integration tests (architecture-strategist):** The plan adds
**unit tests** for `offset_to_pointer` in `parse.rs` and `lookup_hover_content`
in `schema.rs` (Steps 3-4). These test the subtle edge cases (whitespace,
structural tokens, `$ref` cycles) with fast, targeted assertions. Integration
tests then verify the full LSP round-trip with fewer, broader cases.

</details>

## Technical Considerations

### Architecture

- The `hover` handler follows the same async pattern as validation: snapshot
  document from `document_map`, do work, return result.
- Hover bypasses `validate.rs` -- direct path from `lsp.rs` -> `parse.rs` +
  `schema.rs`. This is correct since hover does not validate.
- Schema resolution reuses `resolve_schema_for_document` (same as validation
  path), ensuring hover and diagnostics always agree on which schema applies.
- Raw schema JSON is stored as `Arc<serde_json::Value>` in `SchemaCache`, adding
  ~one clone of the parsed JSON per schema. Memory overhead is bounded by the
  number of distinct schemas in use.
- `lookup_hover_content` lives in `schema.rs` for now. Extract to
  `schema_walk.rs` when completions (#13) or additional annotation types are
  added.

### Performance

- **No `spawn_blocking`** for hover. The workload (parse + AST walk + schema
  walk) completes in <10ms for typical files. `spawn_blocking` adds scheduling
  overhead for no benefit.
- Schema loading from cache is an `Arc` clone (cheap). The schema is almost
  always cached by the time users hover (validation runs on `did_open`).
- Re-parsing the document on each hover is acceptable for v1. For a 5,000-line
  JSON file (~200KB), parsing takes ~3-8ms. If profiling shows this is a
  bottleneck, cache the parsed AST in `document_map` invalidated on
  `did_change`.

**Future performance optimizations (not for v1):**

- Cache parsed AST + `line_starts` in `document_map` to eliminate re-parsing
- Use `Arc<String>` for document text in `document_map` to avoid full-text
  clones on snapshot
- Add `SchemaCache::get_if_ready()` non-blocking accessor that returns `None` if
  schema isn't compiled yet (avoids blocking hover on first-open schema
  compilation)

### Concurrency

- Document text is snapshotted from `document_map` at the start of the handler,
  then lock is released. If the document changes mid-hover, the result may be
  slightly stale — this is acceptable (user will re-hover).
- `SchemaCache` access is already thread-safe via `Mutex`.
- Mutex ordering: always release `document_map` before accessing `config_cache`
  or `SchemaCache`. This matches validation's lock ordering.

### Security

- **`$ref` injection:** Only follow fragment-only `$ref` values (starting with
  `#`). Reject external URLs and file paths. The function walks in-memory
  `serde_json::Value` and must never issue network requests or file reads.
- **Annotation truncation:** Truncate `title`/`description` to 10,000 chars to
  prevent oversized hover responses from malicious schemas.
- **Input clamping:** Apply existing defensive patterns (`line_starts.get()`,
  `.min(line.len())`) for out-of-bounds LSP positions. Return `Ok(None)` rather
  than panicking.
- **Overall risk:** LOW. Rust's memory safety eliminates buffer overflows.
  Local-only LSP over stdin/stdout. No new attack surface beyond schema content
  display (same as VS Code's built-in JSON LSP).

### Deferred scope

- `allOf`/`anyOf`/`oneOf` composition keyword traversal
- `additionalProperties` fallback for unmatched keys
- `patternProperties` matching (regex-based key lookup)
- `if`/`then`/`else` conditional schema resolution
- `$dynamicRef` / `$recursiveRef`
- Draft-aware `$ref` sibling keyword handling (draft-07 vs 2019-09+)
- Showing additional annotations (`type`, `enum`, `default`, `examples`,
  `deprecated`)
- `MarkupKind` negotiation (always use Markdown for now)
- Hover on root `{` showing top-level schema description
- Cached AST in `document_map` for performance
- `Arc<String>` document text optimization
- Non-blocking `SchemaCache::get_if_ready()` accessor

## Acceptance Criteria

- [x] Hovering over a JSON key shows `title` and `description` from the schema
- [x] Hovering over a value shows the same annotation as its key
- [x] Hover returns `null` (no popup) when no schema annotation exists
- [x] `$ref` fragment references followed (with visited-set cycle detection)
- [x] Non-fragment `$ref` values are ignored (no network requests or file reads)
- [x] Works for both `$schema`-field and `jvl.json`-mapping schemas
- [x] Array elements resolve via `items` / `prefixItems`
- [x] UTF-8 and UTF-16 position encoding both work correctly
- [x] Graceful degradation: parse errors, missing schema, no annotations all
      return `null`
- [x] Unit tests for `offset_to_pointer` and `lookup_hover_content`
- [x] Integration tests cover core hover scenarios (6 tests)

## Dependencies & Risks

- **Shared primitive with #13:** The `offset_to_pointer` function in `parse.rs`
  will also be needed for completions. Design for hover now; refactor when
  completions needs it.
- **Schema memory:** Storing raw `serde_json::Value` increases memory per cached
  schema. For a 4MB schema, expect ~16MB additional heap. Typical schemas are
  much smaller. Acceptable for a desktop LSP.
- **Schema traversal limitations:** v1 does not handle `allOf`/`anyOf`/`oneOf`
  or `additionalProperties`. Properties covered by these keywords will show no
  hover. This is a known, documented limitation that can be addressed
  incrementally.

## Affected Files

| File                         | Changes                                                                                                                |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `src/schema.rs`              | Store raw `serde_json::Value` in `SlotResult`; add `get_schema_value` accessor; add `lookup_hover_content`; unit tests |
| `src/parse.rs`               | Add `offset_to_pointer` (byte offset -> JSON pointer path + node range); unit tests                                    |
| `src/lsp.rs`                 | Add `hover_provider` capability; implement `hover()` handler; add `lsp_col_to_byte`                                    |
| `tests/common/lsp_client.rs` | Add `hover()` request helper to `TestClient`                                                                           |
| `tests/lsp_hover.rs`         | New integration test file (6 tests)                                                                                    |
| `tests/fixtures/`            | New schema + JSON fixtures for hover tests                                                                             |

## Sources & References

- GitHub issue: [#14](https://github.com/sargunv/jvl/issues/14)
- Related issue: [#13](https://github.com/sargunv/jvl/issues/13) (completions,
  shares position-to-pointer)
- LSP capability: `src/lsp.rs:307-319`
- SchemaCache: `src/schema.rs:473-611`
- Position utilities: `src/parse.rs:184-205`
- Test client: `tests/common/lsp_client.rs`
- Existing LSP tests: `tests/lsp_*.rs`
- tower-lsp hover trait: `tower_lsp_server::LanguageServer::hover` ->
  `Result<Option<Hover>>`
- LSP types:
  `HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value })`
- LSP spec:
  https://microsoft.github.io/language-server-protocol/specification#textDocument_hover
