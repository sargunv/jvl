---
status: pending
priority: p3
issue_id: "006"
tags: [rust, testing]
dependencies: []
---

# Assert diagnostic severity in strict mode test

## Problem Statement

`strict_mode_no_schema_produces_diagnostic` asserts `code` and `source` but not
`severity`. Since `validate.rs` hardcodes `Severity::Error` for `no-schema`,
asserting `severity == 1` (DiagnosticSeverity::ERROR) would pin that contract.
The existing parse error test has the same gap.

## Findings

- `tests/lsp_diagnostics.rs:197` — asserts code and source but not severity
- `src/validate.rs:88` — hardcodes `Severity::Error` for no-schema diagnostic
- LSP maps `Severity::Error` to `DiagnosticSeverity::ERROR` (value 1)
- Found by: pattern-recognition-specialist

## Proposed Solutions

### Option 1: Add severity assertion

**Approach:** Add `assert_eq!(diagnostics[0]["severity"], 1);` to the test.

**Effort:** 5 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Acceptance Criteria

- [ ] `strict_mode_no_schema_produces_diagnostic` test asserts severity == 1

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (pattern-recognition-specialist)
