---
status: pending
priority: p2
issue_id: "003"
tags: [rust, lsp, code-quality]
dependencies: []
---

# Add factory methods to ResolvedDocument

## Problem Statement

`ResolvedDocument { schema_source: None, strict: false, config_log: ... }` is
spelled out 5 times across the early-return paths in
`resolve_schema_for_document`. The codebase already uses factory methods for
this pattern (`FileResult::skipped()`, `FileResult::invalid()`,
`FileResult::tool_error()`). If a field is ever added to `ResolvedDocument`, all
5 sites need updating.

## Findings

- 5 identical or near-identical struct literal sites in
  `src/lsp.rs:373-377, 393-400, 409-416, 423-430, 460-464`
- All share `schema_source: None, strict: false`; only `config_log` varies
  (`None` or `Some(msg)`)
- `FileResult` in `diagnostic.rs` already demonstrates the factory method
  pattern
- Found by: pattern-recognition-specialist

## Proposed Solutions

### Option 1: Add `skip()` and `error(msg)` factory methods

**Approach:**

```rust
impl ResolvedDocument {
    fn skip() -> Self {
        Self { schema_source: None, strict: false, config_log: None }
    }
    fn error(msg: String) -> Self {
        Self { schema_source: None, strict: false, config_log: Some(msg) }
    }
}
```

**Pros:**

- Matches existing `FileResult` pattern
- Reduces duplication from 5 multi-line literals to single-line calls
- Future field additions only need updating in 2 places

**Cons:**

- Minor: adds 8 lines of impl to save ~20 lines of literals

**Effort:** 15 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**

- `src/lsp.rs:373-377, 393-400, 409-416, 423-430, 460-464` — replace struct
  literals with factory calls

## Resources

- **PR:** #19
- **Pattern precedent:** `FileResult::skipped()`, `FileResult::invalid()` in
  `src/diagnostic.rs`

## Acceptance Criteria

- [ ] `ResolvedDocument::skip()` and `ResolvedDocument::error(msg)` factory
      methods added
- [ ] All 5 early-return sites use factory methods
- [ ] All tests pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (pattern-recognition-specialist)

**Actions:**

- Identified 5 repeated struct literal sites
- Noted existing factory method pattern in FileResult
