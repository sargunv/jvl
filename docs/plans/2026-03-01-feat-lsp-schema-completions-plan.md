---
title: "feat: Schema-driven textDocument/completion"
type: feat
status: completed
date: 2026-03-01
deepened: 2026-03-01
---

# Schema-driven textDocument/completion

## Enhancement Summary

**Deepened on:** 2026-03-01 **Research agents used:** pattern-recognition,
performance-oracle, code-simplicity, architecture-strategist,
best-practices-researcher, framework-docs-researcher, Context7

### Key Improvements

1. Simplified to a single text-scanning context resolution function (eliminates
   dual-function complexity)
2. Stale value cache updated from `validate_and_publish` (avoids adding parse
   latency to `did_change`)
3. Extracted shared helpers for position conversion and schema resolution
   (reduces hover/completion duplication)
4. `allOf` support added to shared `resolve_subschema` (benefits both hover and
   completion)
5. `PropertyInfo` trimmed to 4 fields (removed unused `default_value`, moved
   enum/const to value path)
6. `textEdit` preferred over `insertText` for more precise editing (per
   vscode-json-languageservice patterns)

### New Considerations Discovered

- `jsonc-parser` lenient options (`allow_missing_commas`, etc.) do NOT help with
  malformed documents — verified empirically
- vscode-json-languageservice uses an error-tolerant parser; since we can't, the
  stale value cache is the right workaround
- Property key completions should include value placeholder with
  type-appropriate snippet (e.g., `"name": "$1"`)
- Recursion depth limit (32) needed for `allOf` traversal to handle pathological
  schemas

## Overview

Add schema-driven completions to the jvl LSP. When a user opens a JSON/JSONC
file with an associated schema (via `$schema` field or jvl.json mapping), the
LSP should suggest property names, enum values, and boolean/null literals based
on the schema at the cursor position.

This follows the same handler pattern established by `textDocument/hover` (PR
#21) and reuses existing infrastructure: `offset_to_pointer`,
`resolve_subschema`, `SchemaCache::get_or_compile_with_value`, and the
UTF-8/UTF-16 position encoding negotiation.

GitHub issue: [#13](https://github.com/sargunv/jvl/issues/13)

## Problem Statement

The LSP currently only provides diagnostics and hover. Without completions,
users must consult the schema manually to discover valid property names, enum
values, and required fields. Completions are the highest-impact editor feature
after diagnostics.

## Proposed Solution

Implement property-name completions first (highest value/effort ratio), then
extend to value completions (enum, const, boolean, null). This matches **Option
2** from the issue, extended with value completions once the core infrastructure
is in place.

The key new capability needed beyond what hover provides is **cursor context
resolution** — determining what the cursor is inside when there is no node at
the cursor position (e.g., after `{` or `,` in an object).

## Technical Approach

### 1. Cursor Context Resolution (new: `src/parse.rs`)

The hover handler uses `offset_to_pointer` which returns `None` for whitespace
and structural tokens. Completions need to work at exactly those positions.

Use a **single text-scanning function** for all cases. The cursor context
question ("am I at a key position or a value position?") is fundamentally about
the text around the cursor — scanning backward for `{`, `,`, `:` while
respecting strings and comments. This works regardless of whether the AST is
available.

```rust
/// Result of analyzing cursor context for completions.
pub enum CompletionContext {
    /// Cursor is in a position where a property key should go.
    PropertyKey {
        /// Nesting depth (0 = root object). Used with stale value to find container.
        nesting_depth: usize,
    },
    /// Cursor is in a position where a property value should go.
    PropertyValue {
        /// The property name whose value is being completed.
        property_name: String,
        /// Nesting depth of the containing object.
        nesting_depth: usize,
    },
}

/// Determine the completion context at a byte offset by scanning the source text.
/// Returns `None` if the cursor is inside a comment, inside a non-completable
/// string, or at the document root outside any object/array.
pub fn completion_context(source: &str, byte_offset: usize) -> Option<CompletionContext>
```

**Algorithm outline:**

1. Scan backward from `byte_offset` through the source text using a simple state
   machine that tracks string boundaries (watching for `"` while handling `\"`
   escapes) and comments (`//`, `/* */`).
2. If currently inside a comment → return `None`.
3. Count brace nesting depth as we scan backward to determine which object level
   we're in.
4. Find the nearest unmatched `{`:
   - If the closest non-whitespace before the cursor (outside strings) is `{` or
     `,` → `PropertyKey`.
   - If the closest non-whitespace is `:` → `PropertyValue` (extract property
     name from the preceding key token).
   - If inside a string that is a property key (preceded by `{` or `,` at the
     same nesting level) → `PropertyKey`.
   - If inside a string that is a value (preceded by `:`) → `PropertyValue`.
5. Return `CompletionContext` with the nesting depth.

The `existing_keys` and `container_pointer` are determined separately from the
`serde_json::Value` (fresh parse or stale cache), not from this text scan.

### Research Insights: Context Resolution

**Best practice (vscode-json-languageservice):** The reference implementation
uses `getNodeFromOffset()` on an error-tolerant AST, then inspects the node type
and parent to determine key vs value position. Since `jsonc-parser` v0.29 has no
error recovery (verified: lenient `ParseOptions` like `allow_missing_commas` do
not help with unterminated strings, missing values, or unclosed objects), our
text-scanning approach is the correct alternative.

**Edge cases to handle in the scanner:**

- `{` inside a string literal → must not be counted as structural
- `//` and `/* */` comments containing `{`, `:`, `,` → must be skipped
- Escaped quotes `\"` inside strings → must not end the string
- Multi-line comments spanning the cursor position
- Cursor immediately on `}` or `]` → should advance to parent context (no
  completions at closing braces)

### 2. Schema Property Collection (new: `src/schema.rs`)

Extend schema walking to collect completable properties from a subschema:

```rust
pub struct PropertyInfo {
    pub name: String,
    pub required: bool,
    pub description: Option<String>,  // title + description merged for display
    pub schema_type: Option<String>,  // e.g. "string", "number", "object"
}

/// Collect all completable properties from a schema at the given pointer path.
/// Follows $ref, merges allOf. Returns empty vec if not an object schema.
pub fn collect_properties(
    root: &serde_json::Value,
    pointer: &[String],
) -> Vec<PropertyInfo>

/// Possible value suggestions for a property.
pub enum ValueSuggestion {
    Enum(serde_json::Value),     // from schema `enum`
    Const(serde_json::Value),    // from schema `const`
    Boolean,                      // suggests true/false
    Null,                         // suggests null
}

/// Collect possible values for a property at the given pointer path.
pub fn collect_values(
    root: &serde_json::Value,
    pointer: &[String],
    property_name: &str,
) -> Vec<ValueSuggestion>
```

**Schema composition handling:**

- `$ref`: Follow using existing `follow_ref` with cycle detection.
- `allOf`: Merge `properties` from all subschemas (union of keys). Merge
  `required` arrays. First occurrence wins for metadata.
- `anyOf`/`oneOf`: Out of scope for initial implementation (defer to follow-up).
- `if/then/else`: Out of scope for initial implementation.

### Research Insights: Schema Composition

**Best practice (vscode-json-languageservice):** Collects properties from ALL
`allOf`/`anyOf`/`oneOf` branches via `getMatchingSchemas()` and deduplicates by
property name. For `oneOf` with a discriminator, narrows to the matching branch
after the discriminator value is set. Starting with allOf-only and deferring
anyOf/oneOf is a sound incremental approach.

**Recursion depth limit:** Add a depth limit of 32 for `allOf` traversal as
defense against pathological schemas. Kubernetes-scale schemas typically have
2-5 levels of `allOf` nesting, well within bounds.

**Also extend `resolve_subschema` to handle `allOf`:** Currently,
`resolve_subschema` (used by hover) does not handle `allOf`. If a schema defines
properties only within an `allOf` branch, hover will not find annotations.
Extending `resolve_subschema` to walk `allOf` branches ensures consistency:
completions and hover use the same schema navigation logic.

### 3. Shared Helpers (refactor: `src/lsp.rs`)

Before adding the completion handler, extract duplicated boilerplate from the
hover handler into shared helpers:

```rust
/// Convert an LSP Position to a byte offset in the source text.
fn lsp_position_to_byte_offset(
    content: &str,
    line_starts: &[usize],
    position: Position,
    utf8: bool,
) -> Option<usize>

/// Resolve the schema value for a document URI.
/// Returns None if the URI is not a file:// URI, no schema is configured,
/// or the schema fails to compile.
fn resolve_schema_value(
    &self,
    uri: &Uri,
    parsed_value: &serde_json::Value,
) -> Option<Arc<serde_json::Value>>
```

Also extract a public `resolve_subschema_at_pointer` in `schema.rs`:

```rust
/// Navigate to a subschema at a JSON pointer path, following $ref and allOf.
/// Public entry point that initializes cycle-detection state internally.
pub fn resolve_subschema_at_pointer<'a>(
    root: &'a serde_json::Value,
    pointer: &[String],
) -> Option<&'a serde_json::Value>
```

### Research Insights: Shared Helpers

**Pattern review finding:** The hover handler (lines 444-457, 465-486 of
`src/lsp.rs`) contains ~25 lines of position conversion and schema resolution
that the completion handler would duplicate verbatim. Extracting these prevents
the duplication from growing as more handlers are added (e.g.,
`textDocument/definition` for jumping to schema locations).

### 4. ServerCapabilities Registration (`src/lsp.rs`)

In `initialize`, add:

```rust
completion_provider: Some(CompletionOptions {
    trigger_characters: Some(vec!["\"".to_string()]),
    resolve_provider: Some(false),
    ..Default::default()
}),
```

**Trigger character:** `"` only. Manual invocation (Ctrl+Space) handles all
other positions. The vscode-json-languageservice does not even register trigger
characters — VS Code triggers on identifier characters automatically.
Registering `"` is a pragmatic middle ground.

**Client capability checks** in `initialize`:

- `textDocument.completion.completionItem.snippetSupport` → store in `Backend`
  as `AtomicBool`.
- Reuse existing `hover_markdown` for `documentation` format (completion and
  hover share the same client).

### 5. Completion Handler (`src/lsp.rs`)

Follow the hover handler pattern, using shared helpers:

```rust
async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
    // 1. Extract URI, check file:// scheme
    let uri = &params.text_document_position.text_document.uri;

    // 2. Snapshot document content from document_map
    let content = { /* lock, clone Arc, release */ };

    // 3. Try parsing current text
    let parsed_value = match parse::parse_jsonc(&content) {
        Ok(p) => p.value,
        Err(_) => {
            // Fall back to stale cached value
            match self.last_good_value.lock().get(uri).cloned() {
                Some(v) => (*v).clone(),
                None => return Ok(None), // No stale value available
            }
        }
    };

    // 4. Convert LSP position → byte offset
    let byte_offset = lsp_position_to_byte_offset(
        &content, &line_starts, position, utf8
    )?;

    // 5. Determine CompletionContext via text-scanning
    let ctx = match parse::completion_context(&content, byte_offset) {
        Some(ctx) => ctx,
        None => return Ok(None),
    };

    // 6. Resolve schema value (shared helper)
    let schema_value = match self.resolve_schema_value(uri, &parsed_value) {
        Some(v) => v,
        None => return Ok(None),
    };

    // 7. Determine pointer path from nesting depth + parsed_value
    let pointer = value_pointer_at_depth(&parsed_value, ctx.nesting_depth());

    // 8. Build completion items based on context
    let items = match ctx {
        CompletionContext::PropertyKey { .. } => {
            let props = schema::collect_properties(&schema_value, &pointer);
            let existing = existing_keys_at_depth(&parsed_value, ctx.nesting_depth());
            build_property_items(props, &existing, snippet_support)
        }
        CompletionContext::PropertyValue { property_name, .. } => {
            let values = schema::collect_values(&schema_value, &pointer, &property_name);
            build_value_items(values)
        }
    };

    // 9. Return
    Ok(Some(CompletionResponse::List(CompletionList {
        is_incomplete: false,
        items,
    })))
}
```

### 6. CompletionItem Construction

**For property keys:**

- `label`: property name (e.g., `name`)
- `kind`: `CompletionItemKind::PROPERTY`
- `detail`: type string + `" (required)"` if required
- `documentation`: schema `description` (Markdown or PlainText per
  `hover_markdown`)
- `sort_text`: `"0_name"` for required, `"1_name"` for optional
- `insert_text_format`: `InsertTextFormat::SNIPPET` if client supports it
- `insert_text`: type-appropriate snippet (see below)

**For enum/const values:**

- `label`: value as string (e.g., `"dark"`, `42`, `true`)
- `kind`: `CompletionItemKind::ENUM_MEMBER` for enum,
  `CompletionItemKind::VALUE` for boolean/null
- `insert_text`: the literal value

**Insert text with value placeholders (per vscode-json-languageservice
pattern):**

When the client supports snippets, property key completions include a
type-appropriate value placeholder:

```rust
fn snippet_for_type(schema_type: Option<&str>) -> &str {
    match schema_type {
        Some("string") => "\"$1\"",
        Some("number") | Some("integer") => "${1:0}",
        Some("boolean") => "${1:false}",
        Some("object") => "{$1}",
        Some("array") => "[$1]",
        Some("null") => "null",
        _ => "$0",
    }
}

// Full insert text for a property key:
// "propertyName": <snippet>
```

When snippets are not supported, insert `"propertyName":` with a trailing space.

### Research Insights: CompletionItem

**Best practice:** Use `textEdit` (not `insertText`) for more precise control
over what text gets replaced. `insertText` is subject to client-side
interpretation. However, `textEdit` requires knowing the exact range to replace,
which depends on the cursor position relative to existing text. For the initial
implementation, `insertText` is simpler and works well enough. Migrate to
`textEdit` if edge cases arise.

**CompletionItemKind values (per vscode-json-languageservice):**

- Property keys: `PROPERTY` (10)
- Enum values: `ENUM_MEMBER` (20)
- Boolean/null: `VALUE` (12)

### 7. Malformed Document Strategy

**Problem:** `jsonc-parser` v0.29 has **no error recovery**. When the user is
mid-typing (e.g., `{"na` — an unterminated string), `parse_to_ast` returns `Err`
with no partial AST. Lenient `ParseOptions` (`allow_missing_commas`,
`allow_loose_object_property_names`) were tested and do not help — the parser
still fails on all critical mid-typing states.

Verified failure cases (all return `Err`, no AST, regardless of options):

- `{"na` — unterminated string (typing a key)
- `{"name"` — key without colon
- `{"name":` — colon without value
- `{"name": "Alice",` — trailing content, unclosed object
- `{` — just open brace

Only these produce a valid AST:

- `{}` — empty object
- `{"name": "Alice", }` — trailing comma (valid JSONC)

**Strategy: Cache the last successfully parsed `serde_json::Value` per
document.**

Store `serde_json::Value` (owned, no lifetime issues). Update the cache from
within `validate_and_publish` after the debounced parse succeeds — not
synchronously in `did_change` — to avoid adding parse latency to the keystroke
hot path. The cache may lag by up to 200ms (the debounce interval), which is
acceptable for a fallback.

```rust
// In Backend:
last_good_value: Arc<Mutex<HashMap<Uri, Arc<serde_json::Value>>>>,
```

The completion handler uses this fallback value for:

- Schema resolution (needs `$schema` field)
- Existing-key collection (filter already-present properties)
- Container path determination (nesting depth → JSON pointer)

**Limitations:**

- New nested objects added since the last valid parse won't have completions
  until the document becomes valid again.
- Property filtering uses stale key list, so just-added properties may still
  appear.
- A file opened for the first time in an invalid state has no stale value —
  completions return empty.
- These are acceptable: completions degrade gracefully, and the document returns
  to a valid state frequently.

### Research Insights: Malformed Document Strategy

**Performance finding:** The `last_good_value` should be updated from
`validate_and_publish` (the async debounced validation task), NOT from
`did_change` directly. The current `did_change` handler only stores the text and
spawns validation — it does not parse. Adding a synchronous parse to
`did_change` would add latency to every keystroke. Updating from
`validate_and_publish` keeps the existing async flow intact.

**Simplification finding:** Consider combining `last_good_value` into the
`document_map` entry as
`HashMap<Uri, (i32, Arc<String>, Option<Arc<serde_json::Value>>)>`. This reduces
lock acquisitions and eliminates the risk of the two maps getting out of sync on
`did_close`. The downside is a slightly larger critical section, but the
additional work is just an `Option` assignment.

**Memory:** A `serde_json::Value` for a typical 5KB config is ~10-25KB. With 20
open docs, this is ~200-500KB total — negligible.

## Acceptance Criteria

- [x] `textDocument/completion` requests return property-name suggestions for
      object positions
- [x] Suggestions include `description` from schema as documentation
- [x] Required properties sort before optional properties
- [x] Already-present properties are filtered from suggestions
- [x] `$ref` references are followed for property lookup
- [x] `allOf` subschemas are merged for property collection
- [x] Enum/const value completions at value positions
- [x] Boolean-typed properties suggest `true`/`false`
- [x] Completions return empty (not error) for: no schema, parse failure (with
      no stale cache), inside comments, non-file URIs
- [x] Completions work via stale cache when document is currently malformed
- [x] Integration tests cover: empty object, nested object, $ref properties,
      enum values, existing-key filtering, malformed-doc fallback

## System-Wide Impact

- **Interaction graph**: Completion handler reads `document_map` and
  `schema_cache` (read-only). `validate_and_publish` gains a write to
  `last_good_value` (or the combined `document_map` entry) on successful parse.
  No other callbacks or side effects.
- **Error propagation**: Parse errors → fall back to stale value → `Ok(None)` if
  no stale value. Schema errors → `Ok(None)`. Only LSP protocol errors propagate
  as `Err`.
- **State lifecycle risks**: `did_close` must clean up `last_good_value` entries
  to prevent unbounded memory growth. If combined into `document_map`, this is
  automatic.
- **API surface parity**: Completion uses the same
  `SchemaCache::get_or_compile_with_value` as hover. Extending
  `resolve_subschema` with `allOf` support benefits both hover and completion.

## Implementation Plan

### Phase 1: Shared Infrastructure Refactors

Extract shared helpers from the hover handler before adding completion code.

1. Extract `lsp_position_to_byte_offset()` helper in `src/lsp.rs` (used by hover
   lines 444-457)
2. Extract `Backend::resolve_schema_value()` helper in `src/lsp.rs` (used by
   hover lines 465-486)
3. Extract `resolve_subschema_at_pointer()` as a public wrapper in
   `src/schema.rs`
4. Extend `resolve_subschema` to follow `allOf` branches (benefits hover too)
5. Refactor the hover handler to use the new helpers (no behavior change)
6. Add `last_good_value` field to `Backend` (or combine into `document_map`
   entry)
7. Update `validate_and_publish` to populate `last_good_value` on successful
   parse
8. Update `did_close` to clean up `last_good_value`

**Files:** `src/lsp.rs`, `src/schema.rs` **Estimated scope:** ~80 lines
changed/added

### Phase 2: Context Resolution (`src/parse.rs`)

Single text-scanning function for cursor context.

1. Add `CompletionContext` enum with `PropertyKey` and `PropertyValue` variants
2. Implement `completion_context(source, byte_offset)` with backward text
   scanning:
   - State machine tracking string boundaries and comment regions
   - Brace nesting depth counting
   - Key vs value discrimination based on nearest structural token
3. Unit tests for: empty object `{|}`, after comma `{"a": 1, |}`, after colon
   `{"a": |}`, nested object, inside string key, inside string value, inside
   comment, cursor on `}`, cursor in array

**Files:** `src/parse.rs` **Estimated scope:** ~150 lines

### Phase 3: Schema Property & Value Collection (`src/schema.rs`)

1. Add `PropertyInfo` struct (4 fields: name, required, description,
   schema_type)
2. Add `ValueSuggestion` enum (Enum, Const, Boolean, Null)
3. Implement `collect_properties`: navigate to subschema, extract `properties`
   keys with metadata, merge `allOf` branches, depth limit 32
4. Implement `collect_values`: extract `enum`, `const`, infer boolean/null from
   `type`
5. Helper: `value_pointer_at_depth(value, depth) -> Vec<String>` — walk
   `serde_json::Value` to find the object at a given nesting depth
6. Helper: `existing_keys_at_depth(value, depth) -> Vec<String>` — collect
   property names from the value at the given depth
7. Unit tests for: flat properties, $ref properties, allOf merge, enum values,
   boolean type, nested pointer resolution

**Files:** `src/schema.rs` **Estimated scope:** ~200 lines

### Phase 4: LSP Handler (`src/lsp.rs`)

1. Add `completion_provider` to `ServerCapabilities` in `initialize`
2. Check `snippetSupport` client capability, store as `AtomicBool`
3. Implement `completion` handler:
   - Use shared helpers for position conversion and schema resolution
   - Try fresh parse, fall back to stale `last_good_value`
   - Call `completion_context` on source text
   - Call `collect_properties` or `collect_values` based on context
   - Build `CompletionItem` list with appropriate kind, detail, documentation,
     insert_text
4. Helper: `build_property_items()` — converts `PropertyInfo` list to
   `CompletionItem` list
5. Helper: `build_value_items()` — converts `ValueSuggestion` list to
   `CompletionItem` list

**Files:** `src/lsp.rs` **Estimated scope:** ~150 lines

### Phase 5: Integration Tests

1. Add `completion` method to `TestClient` (`tests/common/lsp_client.rs`)
2. Create `tests/fixtures/completion-schema.json` with:
   - Properties with `title`, `description`, various types
   - `required` array
   - `$ref` to `$defs`
   - `allOf` merging
   - `enum` values on a property
   - Nested object properties
3. Write integration tests:
   - Property name completion in empty object
   - Property name completion after comma
   - Enum value completion
   - Boolean value completion
   - Required properties sorted first
   - Already-present properties filtered
   - Nested object completions
   - `$ref` property resolution
   - `allOf` merged properties
   - No-schema document returns empty
   - Malformed document uses stale cache
4. Follow `tests/lsp_hover.rs` pattern: `open_and_wait`, calculate offsets,
   assert on response

**Files:** `tests/lsp_completion.rs`, `tests/common/lsp_client.rs`,
`tests/fixtures/completion-schema.json` **Estimated scope:** ~300 lines

## Out of Scope (Future Work)

- `anyOf`/`oneOf` property merging (collect from all branches, deduplicate)
- `oneOf` discriminator narrowing (show only matching branch after discriminator
  is set)
- `if/then/else` conditional properties
- `patternProperties` (regex-based keys can't be completed)
- `$schema` URL completion
- `completionItem/resolve` for deferred documentation (not needed — schema
  lookups are cheap)
- `textEdit` instead of `insertText` (more precise range control, add if edge
  cases arise)
- `default` value pre-fill in insert text
- `additionalProperties` schema for arbitrary keys
- Comma-awareness in insert text (scan forward to determine if trailing comma is
  needed)
- `ArrayElement` completion context (array value completions based on `items`
  schema)
- Full AST caching with self-referential struct (only `serde_json::Value` is
  cached)

## Sources & References

- GitHub issue:
  [#13 — Schema-driven completions](https://github.com/sargunv/jvl/issues/13)
- Hover implementation (pattern to follow): `src/lsp.rs:424-510`
- Schema walking infrastructure: `src/schema.rs:728-807` (`resolve_subschema`,
  `follow_ref`)
- AST position mapping: `src/parse.rs:210-258` (`offset_to_pointer`,
  `offset_to_pointer_walk`)
- Test patterns: `tests/lsp_hover.rs`
- LSP 3.17 completion spec:
  [textDocument/completion](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion)
- Reference implementation:
  [vscode-json-languageservice](https://github.com/microsoft/vscode-json-languageservice)
  — `doComplete()`, `getInsertTextForProperty()`, `getMatchingSchemas()`
- tower-lsp-server 0.23 API: `LanguageServer::completion()` →
  `Result<Option<CompletionResponse>>`
- ls-types: `CompletionItem`, `CompletionResponse`, `CompletionList`,
  `CompletionOptions`, `CompletionItemKind`
