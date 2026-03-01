---
status: resolved
priority: p3
issue_id: "004"
tags: [code-review, architecture, rust]
dependencies: []
---

# Extract shared schema resolution helper for hover and validate paths

## Problem Statement

The two-phase schema resolution (config mapping -> `$schema` field fallback) is
duplicated between the hover handler (`src/lsp.rs` lines 452-467) and
`validate_file` (`src/validate.rs` lines 74-82). When completions (#13) lands,
this will be a third copy.

## Findings

- Hover: `resolve_schema_for_document` + fallback to `extract_schema_field` +
  `resolve_schema_ref` (lsp.rs:452-467)
- Validation:
  `schema_source.or_else(|| extract_schema_field(...).map(|r| resolve_schema_ref(r, base_dir)))`
  (validate.rs:74-82)
- Same resolution order, slightly different code structure
- Found by: Architecture Strategist

## Proposed Solutions

### Option 1: Extract shared free function

**Approach:** Create
`fn resolve_effective_schema(file_path: &Path, parsed_value: &serde_json::Value, config_cache: &Mutex<...>) -> Option<SchemaSource>`
callable by both hover and validate.

**Effort:** 30 minutes **Risk:** Low

## Recommended Action

## Acceptance Criteria

- [ ] Shared schema resolution helper extracted
- [ ] Both hover and validate use the shared helper
- [ ] All existing tests still pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (Code Review)

**Actions:**

- Identified schema resolution duplication between hover and validate paths
- Flagged for extraction before completions (#13) adds a third copy
