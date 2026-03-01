---
status: resolved
priority: p2
issue_id: "003"
tags: [code-review, performance, rust]
dependencies: []
---

# Combine get_or_compile + get_schema_value into single SchemaCache call

## Problem Statement

The hover handler (`src/lsp.rs`, lines 471-473) makes two separate calls to
`SchemaCache`, each acquiring and releasing the `slots` mutex:

```rust
let _ = self.schema_cache.get_or_compile(&schema_source, false);
let Some(schema_value) = self.schema_cache.get_schema_value(&schema_source) else { ... };
```

This is redundant -- both calls look up the same slot. A combined method would
eliminate one lock acquisition and close a theoretical eviction race window
between the two calls.

## Findings

- `src/lsp.rs` lines 471-473: two sequential mutex acquisitions on
  `schema_cache.slots`
- `src/schema.rs` `get_or_compile` (line 539): acquires slots mutex, looks
  up/creates slot, initializes via OnceLock
- `src/schema.rs` `get_schema_value` (line 525): acquires slots mutex again,
  looks up same slot, reads schema_value
- The race window is currently unreachable (single-threaded async runtime) but
  is a code smell
- Found by: Performance Oracle, Architecture Strategist, Code Simplicity
  Reviewer

## Proposed Solutions

### Option 1: Return schema_value from get_or_compile

**Approach:** Modify `CompileResult` or `get_or_compile` return type to include
`Option<Arc<serde_json::Value>>`.

**Pros:**

- Single lock acquisition
- No race window
- Cleaner call site

**Cons:**

- Changes existing public API of `get_or_compile`
- Other callers (validate) don't need the schema_value

**Effort:** 30 minutes

**Risk:** Low

---

### Option 2: Add combined get_or_compile_with_value method

**Approach:** Add a new method that returns both the compile result and the
schema value in one lock acquisition. Leave existing `get_or_compile` unchanged.

**Pros:**

- Non-breaking change
- Callers that don't need schema_value are unaffected

**Cons:**

- Slightly more API surface

**Effort:** 20 minutes

**Risk:** Low

## Recommended Action

## Technical Details

**Affected files:**

- `src/schema.rs` - `SchemaCache` impl, `get_or_compile` or new method
- `src/lsp.rs` - hover handler call site (lines 471-473)

## Resources

- **PR:** #21

## Acceptance Criteria

- [ ] Hover handler uses a single SchemaCache call for compile + value retrieval
- [ ] No redundant mutex acquisition
- [ ] All existing tests still pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (Code Review)

**Actions:**

- Identified double mutex acquisition pattern in hover handler
- Proposed two approaches to combine the calls

**Learnings:**

- Race window is currently unreachable but represents a maintenance concern
