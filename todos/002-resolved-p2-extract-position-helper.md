---
status: resolved
priority: p2
issue_id: "002"
tags: [code-review, architecture, rust]
dependencies: []
---

# Extract byte_offset_to_lsp_position helper to DRY range conversion

## Problem Statement

The hover handler (`src/lsp.rs`, lines 481-508) contains 28 lines of
byte-offset-to-LSP-Position conversion logic that is nearly identical to the
same pattern in `file_diagnostic_to_lsp` (lines 719-755). Both perform:
`offset_to_line_col` -> line text lookup -> `byte_col_to_lsp`. This duplication
will grow when completions (#13) adds another consumer.

## Findings

- `src/lsp.rs` hover handler, lines 481-508: converts `node_range.start` and
  `node_range.end` to LSP Position
- `src/lsp.rs` `file_diagnostic_to_lsp`, lines 719-755: does the same conversion
  for diagnostic start/end
- Both follow identical steps: get 1-based line/col, convert to 0-based, look up
  line text, call `byte_col_to_lsp`
- Found by: Architecture Strategist, Code Simplicity Reviewer

## Proposed Solutions

### Option 1: Extract shared helper function

**Approach:** Create
`fn byte_offset_to_lsp_position(source: &str, line_starts: &[usize], offset: usize, utf8: bool) -> Position`
and use it in both hover and diagnostics.

**Pros:**

- Saves ~20 LOC in hover, ~10 LOC in diagnostics
- Single source of truth for offset-to-Position conversion
- Benefits future features (completions #13, code actions)

**Cons:**

- Minor refactor of existing `file_diagnostic_to_lsp`

**Effort:** 30 minutes

**Risk:** Low

## Recommended Action

## Technical Details

**Affected files:**

- `src/lsp.rs` - hover handler range conversion (lines 481-508)
- `src/lsp.rs` - `file_diagnostic_to_lsp` function (lines 719-755)

## Resources

- **PR:** #21
- **Related issue:** #13 (completions will also need this)

## Acceptance Criteria

- [ ] Shared `byte_offset_to_lsp_position` helper extracted
- [ ] Hover handler uses the helper for start/end position
- [ ] `file_diagnostic_to_lsp` uses the helper for start/end position
- [ ] All existing tests still pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (Code Review)

**Actions:**

- Identified duplicated range conversion pattern in hover and diagnostics
- Proposed extraction of shared helper function

**Learnings:**

- Pattern appears in exactly 2 places now, will be 3+ with completions
