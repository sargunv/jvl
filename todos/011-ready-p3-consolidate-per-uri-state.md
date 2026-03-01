---
status: ready
priority: p3
issue_id: "011"
tags: [architecture, lsp]
dependencies: []
---

# Consolidate per-URI state into DocumentState

## Problem Statement

The LSP backend stores per-URI state across multiple separate `HashMap`s
(`documents`, `last_good_value`, `schema_urls`, etc.). This parallel map
structure is error-prone — adding/removing a document requires updating all maps
in sync.

## Findings

- Multiple `HashMap<String, _>` fields in `Backend` or `BackendState` for
  per-document state
- Adding new per-document state requires touching multiple maps
- Risk of maps getting out of sync on document open/close

## Proposed Solutions

### Option A: Create DocumentState struct

Combine all per-URI state into a single `DocumentState` struct stored in one
`HashMap<String, DocumentState>`.

- **Effort:** Medium
- **Risk:** Low

## Acceptance Criteria

- [ ] Single `DocumentState` struct holding all per-document state
- [ ] One `HashMap<String, DocumentState>` replaces multiple maps
- [ ] No behavioral change

## Work Log

### 2026-03-01 - Code review finding

**By:** Claude Code — identified by architecture-strategist agent during PR #22
review.
