---
status: resolved
priority: p1
issue_id: "001"
tags: [code-review, security, rust]
dependencies: []
---

# Fix UTF-8 boundary panic in format_hover truncation

## Problem Statement

The `truncate` closure in `format_hover` (`src/schema.rs`) slices at a raw byte
offset with `&s[..MAX_LEN]`. If byte 10,000 falls in the middle of a multi-byte
UTF-8 character (emoji, CJK), Rust panics with
`byte index 10000 is not a char boundary`, crashing the LSP server process.

This is easily triggered by any schema with a `title` or `description` longer
than 10,000 bytes containing multi-byte characters near the boundary.

## Findings

- `src/schema.rs`, `format_hover` function, `truncate` closure
- Uses `&s[..MAX_LEN]` which is a raw byte slice
- Rust's `str` slicing panics if the index is not on a char boundary
- The LSP server runs as a single process; a panic kills the entire session
- Found by: Security Sentinel, Architecture Strategist, Pattern Recognition
  Specialist

## Proposed Solutions

### Option 1: Use `floor_char_boundary` (Stable since Rust 1.82)

**Approach:** Replace `&s[..MAX_LEN]` with
`&s[..s.floor_char_boundary(MAX_LEN)]`.

```rust
let truncate = |s: &str| -> String {
    if s.len() > MAX_LEN {
        let end = s.floor_char_boundary(MAX_LEN);
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
};
```

**Pros:**

- One-line fix, minimal change
- Standard library method, well-tested
- Idiomatic Rust

**Cons:**

- Requires Rust 1.82+ (check MSRV)

**Effort:** 5 minutes

**Risk:** Low

---

### Option 2: Manual char boundary scan

**Approach:** Scan backward from MAX_LEN until a valid char boundary is found.

```rust
let truncate = |s: &str| -> String {
    if s.len() > MAX_LEN {
        let mut end = MAX_LEN;
        while !s.is_char_boundary(end) { end -= 1; }
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
};
```

**Pros:**

- Works on any Rust version
- Simple logic

**Cons:**

- Slightly more code than Option 1

**Effort:** 5 minutes

**Risk:** Low

## Recommended Action

## Technical Details

**Affected files:**

- `src/schema.rs` - `format_hover` function, `truncate` closure

## Resources

- **PR:** #21
- **Rust docs:** `str::floor_char_boundary`

## Acceptance Criteria

- [ ] Truncation uses a char-boundary-safe method
- [ ] Unit test added for multi-byte truncation edge case
- [ ] All existing tests still pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (Code Review)

**Actions:**

- Identified raw byte slicing in `format_hover` truncation closure
- Confirmed panic scenario with multi-byte characters
- Proposed two fix options

**Learnings:**

- `str::floor_char_boundary` is stable since Rust 1.82
- This is a common Rust pitfall when truncating strings by byte length
