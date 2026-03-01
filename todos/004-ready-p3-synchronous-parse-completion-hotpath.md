---
status: ready
priority: p3
issue_id: "004"
tags: [performance, lsp, completions]
dependencies: []
---

# Synchronous parse on completion hot path

## Problem Statement

The completion handler performs a synchronous `parse_jsonc` +
`completion_context` scan while holding the state lock. For large documents,
this blocks the LSP event loop during typing.

## Findings

- Completion handler re-parses document text synchronously on each request
- State lock held during parse prevents concurrent request handling

## Proposed Solutions

### Option A: Cache parsed completion context

Cache the `completion_context` result alongside the document text, invalidating
on `didChange`.

- **Effort:** Medium
- **Risk:** Low

### Option B: Move parse to background task

Spawn parse work on a background thread and respond when ready.

- **Effort:** Large
- **Risk:** Medium

## Acceptance Criteria

- [ ] Completion requests don't block on full document re-parse for unchanged
      text
- [ ] No regression in completion accuracy

## Work Log

### 2026-03-01 - Code review finding

**By:** Claude Code — identified by performance-oracle agent during PR #22
review.
