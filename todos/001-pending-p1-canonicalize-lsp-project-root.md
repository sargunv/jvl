---
status: pending
priority: p1
issue_id: "001"
tags: [rust, lsp, performance, correctness]
dependencies: []
---

# Canonicalize LSP project_root at cache-fill time

## Problem Statement

The LSP's `resolve_schema_for_document` calls `std::fs::canonicalize(path)` on
every validation (after debounce), which is a syscall that hits disk per
keystroke. Additionally, `project_root` is stored non-canonicalized, causing
`strip_prefix` to silently fail on macOS (where `/tmp` symlinks to
`/private/tmp`) and any system with symlinked paths. When `strip_prefix` fails,
the fallback produces an absolute path that won't match relative glob patterns,
silently suppressing all diagnostics.

The CLI already canonicalizes `project_root` at `main.rs:377` but the LSP does
not.

## Findings

- `src/lsp.rs:404`: `project_root` stored without canonicalization
- `src/lsp.rs:449-456`: `std::fs::canonicalize(path)` called on every validation
  inside `spawn_blocking`
- `src/main.rs:377`: CLI correctly calls
  `std::fs::canonicalize(&project_root).unwrap_or(project_root)`
- On macOS, tempfile paths under `/var/folders/...` canonicalize to
  `/private/var/folders/...`, breaking `strip_prefix` against non-canonical root
- Found by: performance-oracle, security-sentinel, architecture-strategist (3
  independent agents)

## Proposed Solutions

### Option 1: Canonicalize project_root at cache-fill time (recommended)

**Approach:** In the cache-miss branch of `resolve_schema_for_document`,
canonicalize `project_root` before storing it in `CompiledConfig`. Then remove
the per-validation `canonicalize` call and use a simple `strip_prefix`.

```rust
// Cache-miss branch (lsp.rs ~line 404):
let raw_root = config_path.parent().unwrap_or(Path::new("."));
let project_root = std::fs::canonicalize(raw_root)
    .unwrap_or_else(|_| raw_root.to_path_buf());

// Hot path (lsp.rs ~line 449, replacing canonicalize):
let relative = path
    .strip_prefix(&compiled.project_root)
    .map(|r| r.to_string_lossy().to_string())
    .unwrap_or_else(|_| path.to_string_lossy().to_string());
```

**Pros:**

- Moves syscall from O(validations) to O(config cache misses) — effectively once
  per workspace session
- Fixes the symlink correctness bug simultaneously
- Matches the CLI's existing approach

**Cons:**

- None significant

**Effort:** 15 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**

- `src/lsp.rs:404` — canonicalize `project_root` here
- `src/lsp.rs:449-456` — simplify to `strip_prefix` without `canonicalize`

## Resources

- **PR:** #19
- **Related:** CLI canonicalization at `src/main.rs:377`

## Acceptance Criteria

- [ ] `project_root` in `CompiledConfig` is canonicalized at cache-fill time
- [ ] Per-validation `std::fs::canonicalize(path)` call removed from hot path
- [ ] `strip_prefix` works correctly on macOS with symlinked temp directories
- [ ] All existing LSP tests pass (including strict mode tests in tempdir)
- [ ] Behavior matches CLI path resolution

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (review agents)

**Actions:**

- performance-oracle identified per-validation syscall as hot-path issue
- security-sentinel flagged asymmetric canonicalization between CLI and LSP
- architecture-strategist confirmed the macOS tempdir edge case

**Learnings:**

- CLI already does this correctly at main.rs:377
- The fix is straightforward: move canonicalize to cache-fill, simplify hot path
