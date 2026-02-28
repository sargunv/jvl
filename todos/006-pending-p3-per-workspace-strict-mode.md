---
status: pending
priority: p3
issue_id: "006"
tags: [lsp, config, strict]
dependencies: []
---

# Per-workspace strict mode in jvl.json (warn on files with no schema)

## Problem Statement

`validate_file` accepts a `strict` flag that, when `true`, emits a `no-schema`
error for files that have no associated schema (`src/validate.rs:82`). The LSP
hard-codes `strict: false` (`src/lsp.rs:142`), silently skipping all files
without a schema. There is no way for a workspace to opt into "warn me about
files missing a schema" without modifying source code.

Allowing this to be set per-workspace in `jvl.json` would let teams enforce
schema coverage at the editor level, not just in CI.

## Findings

- `validate::validate_file` signature already has `strict: bool`
  (`src/validate.rs:24`).
- The LSP call site is `src/lsp.rs:142`:
  `false, // strict: silent skip for files without schema`.
- `Config` struct is defined in `src/discover.rs`; it is deserialized from
  `jvl.json` via serde.
- `CompiledConfig` wraps `CompiledSchemaMappings` and `project_root`
  (`src/lsp.rs:21`); adding `strict: bool` here would flow naturally to
  `validate_and_publish`.
- The CLI `check` subcommand has a `--strict` flag; `jvl.json` should mirror it.
- Semantics question: should `strict` in `jvl.json` apply only to files matched
  by a mapping, or to _all_ files opened in the workspace? The former is safer.

## Proposed Solutions

### Option 1: Add `strict` boolean field to `jvl.json` Config

Add `strict: bool` (default `false`) to the `Config` struct in `discover.rs`,
expose it through `CompiledConfig`, and pass it to `validate_file` in the LSP.

**Approach:**

1. Add `#[serde(default)] strict: bool` to `Config` in `src/discover.rs`.
2. Propagate through `CompiledConfig` in `src/lsp.rs`.
3. Thread through to `validate_file` call in `validate_and_publish`.

**Pros:**

- Minimal change (3 files, ~10 lines)
- Consistent with CLI `--strict` flag
- Default `false` is backwards-compatible

**Cons:**

- No granularity: all or nothing per workspace

**Effort:** Small (1–2 hours)

**Risk:** Low

---

### Option 2: `strict` as a per-pattern option in schema mappings

Allow each mapping entry in `jvl.json` to carry `strict: true`, so only files
matched by that pattern are checked for schema presence.

**Pros:**

- Fine-grained: enforce schema coverage only for `src/**/*.json` but not
  `fixtures/**`

**Cons:**

- More complex config schema and mapping compilation
- Schema mapping API change required in `discover.rs`

**Effort:** Medium (3–4 hours)

**Risk:** Low–Medium

---

### Option 3: Separate `noSchemaWarning` field (warning not error)

Rather than a `strict` boolean that emits an error, emit a `Severity::Warning`
for files with no schema when a new `warnOnMissingSchema: true` field is set.

**Pros:**

- Warnings don't block CI; errors do — softer opt-in that's more editor-friendly
- Distinct from the CLI `--strict` flag semantics (which is an error)

**Cons:**

- Introduces a second "missing schema" code path
- Naming and semantics diverge from CLI

**Effort:** Small–Medium (2–3 hours)

**Risk:** Low

## Recommended Action

To be filled during triage.

## Technical Details

**Affected files:**

- `src/discover.rs` — `Config` struct (add `strict` field)
- `src/lsp.rs:21` — `CompiledConfig` struct (add `strict` field)
- `src/lsp.rs:142` — `validate_file` call (pass `compiled.strict` instead of
  `false`)
- `src/lsp.rs:392–431` — `resolve_schema_for_document` (return `strict`
  alongside `SchemaSource`)

**Config example:**

```json
{ "strict": true, "schemas": { "*.json": "./my-schema.json" } }
```

**No database changes.**

## Resources

- **PR:** #10
- **Related:** `src/discover.rs` `Config` struct, `src/validate.rs:82`
  `no-schema` error path

## Acceptance Criteria

- [ ] `jvl.json` accepts a `strict` boolean field (default: `false`)
- [ ] When `strict: true`, files opened in the workspace without a schema emit a
      `no-schema` diagnostic in the editor
- [ ] When `strict: false` (default), behaviour is unchanged — files without
      schema are silently skipped
- [ ] `jvl.json` JSON schema (if generated via `schemars`) updated to document
      the field
- [ ] Existing tests pass; new test covering strict mode in LSP context

## Work Log

### 2026-02-28 - Initial capture

**By:** Claude Code

**Actions:**

- Located `strict: false` hard-code at `src/lsp.rs:142`
- Traced `Config` struct in `src/discover.rs` and `CompiledConfig` in
  `src/lsp.rs:21`
- Confirmed `validate_file` already supports the flag — only wiring needed
- Drafted three options (boolean, per-pattern, warning-level variant)

**Learnings:**

- This is almost entirely plumbing; the hard work (`no-schema` error path)
  already exists
