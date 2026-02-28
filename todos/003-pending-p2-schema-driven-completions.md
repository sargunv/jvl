---
status: pending
priority: p2
issue_id: "003"
tags: [lsp, completions, schema]
dependencies: []
---

# Schema-driven completions (textDocument/completion)

## Problem Statement

The LSP currently only provides diagnostics. Editors expect JSON/JSONC
completions driven by the validated schema — property names, enum values,
required fields, etc. Without completions, users can't discover valid keys or
values without consulting the schema manually.

The `jsonschema` crate (0.42) does not expose schema introspection APIs: there
is no public way to ask "what properties does this object allow at this pointer
path?". The schema must be walked as a raw `serde_json::Value`.

## Findings

- `lsp.rs` `initialize` returns `ServerCapabilities` with `..Default::default()`
  for everything except `text_document_sync` and `position_encoding`
  (`src/lsp.rs:226`). Adding
  `completion_provider: Some(CompletionOptions { .. })` opts into completions.
- The cursor position must be converted to a JSON pointer path before looking up
  the schema. `parse.rs` already has `resolve_pointer_in_ast` walking the
  opposite direction (pointer → span); a reverse walk (position → pointer) is
  needed.
- Schema resolution is already done per-document in
  `resolve_schema_for_document` (`src/lsp.rs:360`). The raw `serde_json::Value`
  is available inside `SchemaCache::get_or_compile` but not returned to callers
  — it is currently discarded after compilation.
- `$ref` resolution and `allOf`/`anyOf`/`oneOf` merging must be handled for the
  schema walk to be useful.

## Proposed Solutions

### Option 1: Walk the raw schema `serde_json::Value` at the cursor path

At completion request time:

1. Determine the cursor's JSON pointer path by walking the AST (position →
   pointer).
2. Load the schema as `serde_json::Value` (cache separately or expose from
   `SchemaCache`).
3. Walk the schema value following the pointer to reach the relevant subschema.
4. Collect `properties` keys, `enum` values, `const`, etc. and emit as
   `CompletionItem`s.

**Pros:**

- Full control; no dependency on `jsonschema` internals
- Can handle `description`/`title` annotations for documentation in completion
  items

**Cons:**

- `$ref` resolution, `allOf`, `anyOf`, `oneOf`, `if/then/else` all require
  manual merging
- Significant implementation effort for real-world schemas

**Effort:** Large (2–4 days for solid coverage)

**Risk:** Medium (correctness for complex schemas is hard)

---

### Option 2: Scope to property-name completions only (no value completions)

Implement only the most impactful subset: suggest property names for object
positions, skipping enum/const value completions and complex composition
keywords.

**Pros:**

- Highest value/effort ratio — property name suggestions cover 80% of daily
  usage
- Avoids the `$ref`/`allOf` complexity for a first iteration

**Cons:**

- No enum/value completions
- Still needs position → pointer walk and basic schema descent

**Effort:** Medium (1–2 days)

**Risk:** Low

---

### Option 3: Use a JSON Schema completion library

Evaluate whether an existing Rust or JS (via WASM) library handles schema-driven
completion (e.g., `json-language-server` logic ported or
`vscode-json-languageservice`).

**Pros:**

- Battle-tested with real-world schemas including `$ref` and composition

**Cons:**

- Adds a large dependency or build complexity (WASM)
- May not align with jvl's lightweight philosophy

**Effort:** Unknown (research spike needed)

**Risk:** Medium

## Recommended Action

To be filled during triage.

## Technical Details

**Affected files:**

- `src/lsp.rs` — `initialize` (advertise capability), new `completion` handler
- `src/parse.rs` — new `position_to_pointer` function (cursor offset →
  Vec<String> path)
- `src/schema.rs` — expose raw `serde_json::Value` alongside compiled validator,
  or add a separate `get_raw` method

**New functions needed:**

- `parse.rs`: `position_to_pointer(ast, byte_offset) -> Vec<PathSegment>` —
  walks AST to find what node contains the cursor, returns the pointer path to
  that node
- `schema_walk.rs` (new):
  `subschema_at(root: &Value, path: &[PathSegment]) -> &Value` — walks a schema
  value following a JSON Pointer, resolving `$ref` inline

## Resources

- **PR:** #10
- **jsonschema crate:** no public introspection API as of 0.42
- **LSP spec:** `textDocument/completion`
- **Similar work:** vscode-json-languageservice (TypeScript reference
  implementation)

## Acceptance Criteria

- [ ] Completion requests return property-name suggestions for object positions
- [ ] Completion items include `description`/`title` from schema as
      documentation
- [ ] Enum/const value completions at leaf positions (if full option chosen)
- [ ] `$ref` references are followed (at minimum for top-level definitions)
- [ ] Completions don't appear inside string values or comment nodes

## Work Log

### 2026-02-28 - Initial capture

**By:** Claude Code

**Actions:**

- Confirmed `jsonschema` 0.42 has no public introspection API
- Traced how schema `serde_json::Value` flows through `SchemaCache` (currently
  discarded)
- Identified `position_to_pointer` as the key missing primitive in `parse.rs`
- Outlined three implementation strategies

**Learnings:**

- The AST reverse-walk (position → pointer) is independent of schema logic and
  useful for hover too (see #004) — worth implementing once and sharing
