---
status: resolved
priority: p3
issue_id: "007"
tags: [code-review, performance, rust]
dependencies: []
---

# Use Arc<String> in document_map to avoid full-text clones

## Problem Statement

Every hover request clones the full document `String` from `document_map`
(`src/lsp.rs` line 418). For large documents, this creates unnecessary heap
allocations. Switching to `Arc<String>` makes the snapshot a pointer bump
instead of a full copy. This also benefits the validation path which does the
same clone.

## Findings

- `src/lsp.rs` line 418: `docs.get(&uri).map(|(_, c)| c.clone())` clones full
  String
- `src/lsp.rs` line 138: `validate_and_publish` does the same clone
- `document_map` type is `Arc<Mutex<HashMap<Uri, (i32, String)>>>`
- Changing to `Arc<Mutex<HashMap<Uri, (i32, Arc<String>)>>>` would make clones
  cheap
- Found by: Performance Oracle, Architecture Strategist

## Proposed Solutions

### Option 1: Change document_map value type to Arc<String>

**Approach:** Change `(i32, String)` to `(i32, Arc<String>)` in `document_map`.
Update `did_open` and `did_change` to wrap content in Arc. All consumers get
cheap clones.

**Effort:** 30 minutes **Risk:** Low

## Recommended Action

## Acceptance Criteria

- [ ] `document_map` uses `Arc<String>` for content
- [ ] Hover and validation paths use cheap Arc clones
- [ ] All existing tests still pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (Code Review)

**Actions:**

- Identified full String clone pattern in hover and validation paths
- Proposed Arc<String> to eliminate heap allocation on snapshot
