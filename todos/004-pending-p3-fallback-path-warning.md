---
status: pending
priority: p3
issue_id: "004"
tags: [rust, lsp, observability]
dependencies: ["001"]
---

# Add config_log warning when path fallback is used

## Problem Statement

When `strip_prefix` fails in `resolve_schema_for_document` (e.g., due to
symlinks or unsaved buffers), the code silently falls back to the raw path
string. This absolute path won't match relative glob patterns, causing files to
silently get no diagnostics. There is no log message to help users or developers
diagnose the issue.

## Findings

- `src/lsp.rs:456` — fallback path produces absolute string that won't match
  relative globs
- No log message when fallback is used
- Found by: performance-oracle

## Proposed Solutions

### Option 1: Add config_log warning on fallback

**Approach:** When `strip_prefix` fails, set `config_log` to a warning message
so the LSP logs it to the output panel.

**Effort:** 10 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Acceptance Criteria

- [ ] Warning logged when path relativization falls back
- [ ] Message includes the path and project root for debugging

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (performance-oracle)
