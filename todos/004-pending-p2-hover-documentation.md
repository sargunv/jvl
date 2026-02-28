---
status: pending
priority: p2
issue_id: "004"
tags: [lsp, hover, schema]
dependencies: []
---

# Hover documentation from schema description/title (textDocument/hover)

## Problem Statement

Hovering over a JSON key or value in an LSP-enabled editor should show the
schema's `description` and/or `title` annotation for that location. Currently
the LSP returns nothing for hover requests. This is a high-visibility feature —
it surfaces schema documentation inline without leaving the editor.

## Findings

- `lsp.rs` `initialize` returns `..Default::default()` for capabilities
  (`src/lsp.rs:226`); adding
  `hover_provider: Some(HoverProviderCapability::Simple(true))` opts in.
- The hover handler needs to: (1) convert the cursor position to a JSON pointer
  path, (2) walk the schema value at that path, (3) extract
  `description`/`title` and return them as `MarkupContent`.
- The position → pointer walk is the same primitive needed by completions
  (#003). Worth implementing once in `parse.rs` and reusing.
- The schema `serde_json::Value` must be accessible at hover time. Currently
  `SchemaCache` discards the raw value after compilation (see #003 findings).
- For object keys, the relevant subschema is at `properties.<key>` in the parent
  schema. For array elements, it's `items` (or `prefixItems[i]` in draft
  2020-12).

## Proposed Solutions

### Option 1: Walk raw schema value at cursor's JSON pointer path

1. Convert cursor position to JSON pointer path using `parse.rs`
   position-to-pointer walk.
2. Retrieve raw schema `serde_json::Value` (add to `SchemaCache` return value or
   a separate accessor).
3. Walk the schema to the subschema at that path.
4. Extract `title` and `description` fields, format as Markdown, return in
   `Hover`.

**Pros:**

- Full control over annotation extraction
- Can show `title`, `description`, type info, and enum values in one hover

**Cons:**

- Requires `$ref` resolution for schemas that use definitions
- Shares complexity with #003 (completions)

**Effort:** Medium (1–2 days, less if #003 position-to-pointer work is done
first)

**Risk:** Low for simple schemas; medium for `$ref`-heavy schemas

---

### Option 2: Scope to type/enum information only (no full schema walk)

Show only `type`, `enum` values, and `const` from the subschema — skip `$ref`
and composition keywords for a faster first iteration.

**Pros:**

- Avoids `$ref` resolution complexity
- Still useful for simple schemas

**Cons:**

- Misses `description`/`title` from ref'd definitions, which is where most doc
  strings live

**Effort:** Medium (1 day)

**Risk:** Low

## Recommended Action

To be filled during triage.

## Technical Details

**Affected files:**

- `src/lsp.rs` — `initialize` (advertise capability), new `hover` handler
- `src/parse.rs` — `position_to_pointer` (shared with #003)
- `src/schema.rs` — expose raw `serde_json::Value` alongside compiled validator

**Schema walk for hover:** For a pointer path `["servers", "0", "host"]`:

1. Enter `properties.servers`
2. Enter `items` (or `prefixItems[0]` for draft 2020-12)
3. Enter `properties.host`
4. Read `title`, `description`

**Key vs. value hover:** The cursor might be on the key token (`"host"`) or on
the value. Both should show the same subschema annotation; the
position-to-pointer walk should normalise this.

## Resources

- **PR:** #10
- **LSP spec:** `textDocument/hover`
- **Related:** #003 (completions shares the position-to-pointer primitive)

## Acceptance Criteria

- [ ] Hovering over a JSON key shows `title` and `description` from the schema
- [ ] Hovering over a value shows the same annotation as its key
- [ ] Hover returns `null` (no popup) when no schema annotation exists
- [ ] `$ref` references followed at least one level deep
- [ ] Works for both `$schema`-field and `jvl.json`-mapping schemas

## Work Log

### 2026-02-28 - Initial capture

**By:** Claude Code

**Actions:**

- Traced capability advertisement in `initialize`
- Confirmed raw schema value is discarded after compilation in `SchemaCache`
- Identified position-to-pointer walk as shared primitive with #003
- Proposed minimal fallback (Option 2) as quick win

**Learnings:**

- Option 2 (diagnostic-on-hover) can ship independently of schema walk work and
  is genuinely useful; consider sequencing it first
