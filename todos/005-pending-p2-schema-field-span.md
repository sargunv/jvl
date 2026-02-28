---
status: pending
priority: p2
issue_id: "005"
tags: [lsp, diagnostics, parse]
dependencies: []
---

# Show schema-fetch/compile errors at the $schema field span, not (0,0)

## Problem Statement

When the schema referenced by the `$schema` field fails to load or compile
(network error, bad JSON, compile failure), the resulting `FileDiagnostic` has
`span: None` and `location: None` (`src/validate.rs:120–137`). The LSP converts
a missing location to `Position(0, 0)` — the very first character of the file
(`src/lsp.rs:495`). This is misleading: the error has nothing to do with offset
0; it belongs at the `$schema` field.

## Findings

- `validate_file` emits `span: None` / `location: None` for schema-load and
  schema-compile errors (`src/validate.rs:86–137`).
- `file_diagnostic_to_lsp` in `src/lsp.rs:449` maps `None` location to
  `(0,0)–(0,0)`.
- `parse.rs` already has `resolve_pointer_key` (added in the span-narrowing PR)
  which can return the byte range of a key token given a JSON pointer path.
- `parse::extract_schema_field` (`src/parse.rs:148`) finds the `$schema` value;
  there is no existing function to find the `$schema` key's _span_.
- The fix: after parsing succeeds, call `resolve_pointer_key(["$schema"])` to
  get the key span, and attach it as the location for schema-load/compile
  diagnostics.
- The miette CLI output for these errors already shows `None` span → no
  underline, which is also suboptimal.

## Proposed Solutions

### Option 1: Resolve `$schema` span in `validate_file` and attach to error diagnostics

After `parse_jsonc` succeeds but before/after schema loading fails, call
`parsed.resolve_pointer_key(std::iter::once(LocationSegment::Property("$schema".into())))`
to get the key span, then use it when constructing the error `FileDiagnostic`.

**Pros:**

- Minimal: only changes a few lines in `validate_file`
- No new API needed — `resolve_pointer_key` already exists
- Improves both CLI miette output and LSP squiggle position

**Cons:**

- Only works when a `$schema` field exists in the file (schemas supplied via
  `--schema` flag or `jvl.json` mapping don't have a `$schema` key to point at)

**Effort:** Small (1–2 hours)

**Risk:** Low

---

### Option 2: Point at the `$schema` _value_ span (the URL string)

Use `resolve_pointer` (not `resolve_pointer_key`) on `["$schema"]` to highlight
the URL string itself rather than the key.

**Pros:**

- The URL string is exactly what's wrong (bad URL, unreachable endpoint)
- Slightly more specific than the key

**Cons:**

- For compile errors the URL is correct; the schema _content_ is broken, so
  pointing at the URL is arguably misleading

**Effort:** Same as Option 1

**Risk:** Low

---

### Option 3: Separate span logic per error category

- Schema fetch error → point at `$schema` value (the URL)
- Schema parse/compile error → point at `$schema` key (signals "this schema is
  broken")
- Schema-via-mapping (no `$schema` field) → keep `None` / `(0,0)` with a note in
  message

**Pros:**

- Most precise UX per error type

**Cons:**

- Slightly more code branching

**Effort:** Small–Medium (2–3 hours)

**Risk:** Low

## Recommended Action

To be filled during triage.

## Technical Details

**Affected files:**

- `src/validate.rs:82–137` — error diagnostic construction in `validate_file`
- `src/lsp.rs:449` — `file_diagnostic_to_lsp` (no change needed if span is set
  upstream)

**Key function to use:**

```rust
// In validate_file, after parse succeeds:
let schema_key_span = parsed.resolve_pointer_key(
    std::iter::once(LocationSegment::Property(Cow::Borrowed("$schema")))
);
// Attach as span/location when building the error FileDiagnostic
```

**Edge cases:**

- `$schema` field absent (schema from `--schema` flag or mapping): span stays
  `None`
- `$schema` field present but value is not a string: `resolve_pointer_key`
  returns the key span regardless, which is fine

## Resources

- **PR:** #10
- **LSP spec:** diagnostic range
- **Related:** `src/parse.rs` `resolve_pointer_key` (already implemented)

## Acceptance Criteria

- [ ] Schema-fetch errors show a squiggle under the `$schema` key (or value) in
      the LSP
- [ ] Schema-compile errors similarly point at `$schema`
- [ ] CLI miette output for these errors shows an underline at the `$schema`
      token
- [ ] Files without a `$schema` field (schema from mapping/flag) still work
      correctly
- [ ] Existing tests pass; new snapshot test for the schema-error-with-span case

## Work Log

### 2026-02-28 - Initial capture

**By:** Claude Code

**Actions:**

- Located `span: None` construction in `validate_file` at
  `src/validate.rs:86–137`
- Traced `(0,0)` fallback in `file_diagnostic_to_lsp` at `src/lsp.rs:495`
- Confirmed `resolve_pointer_key` already exists and can resolve `["$schema"]`
- Drafted three options differing in key vs. value targeting

**Learnings:**

- Both the CLI and LSP suffer from this bug — the fix to `validate.rs` improves
  both surfaces simultaneously with no LSP-specific code changes needed
