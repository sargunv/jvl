---
status: pending
priority: p2
issue_id: "002"
tags: [rust, lsp, ux, design-decision]
dependencies: []
---

# Evaluate LSP files glob filtering behavior

## Problem Statement

The `files` glob in `jvl.json` was designed for CLI discovery ("which files
should I walk to?"). This PR applies it to LSP validation too, meaning if a user
sets `"files": ["src/**/*.json"]`, the LSP silently produces no diagnostics for
any JSON file outside `src/`. This was a deliberate design decision during
planning but the code-simplicity-reviewer argues it's semantically wrong for the
LSP and could be a footgun.

The key tension: the CLI discovers files (walks directories), while the LSP
receives files pushed by the editor. The `files` filter means different things
in each context.

## Findings

- `src/lsp.rs:459-465`: `file_filter.matches()` check gates ALL LSP validation,
  not just strict mode
- This was an intentional design choice during planning — user said "Shouldn't
  this be the behavior for the entire LSP?"
- CLI/LSP asymmetry: CLI explicit file args bypass `discover_files` and are
  never checked against `files` globs, so `strict: true` can emit `no-schema`
  for explicitly named files outside the glob. LSP silently skips them.
- code-simplicity-reviewer recommends removing the filter entirely (~55 LOC
  reduction)
- architecture-strategist notes the asymmetry but considers it a documentation
  issue

## Proposed Solutions

### Option 1: Keep current behavior (files filter gates all LSP validation)

**Approach:** Leave as-is. Document clearly that `files` controls both CLI
discovery and LSP validation scope.

**Pros:**

- Consistent: `files` means "these are the files jvl cares about" universally
- Prevents noise: LSP won't show diagnostics for files the user explicitly
  excluded
- Already implemented and tested

**Cons:**

- Surprising: user configures `files` for CLI scope, LSP goes dark for files
  outside it
- Silent failure: no log message when files are filtered out
- CLI/LSP asymmetry: CLI explicit args bypass the filter, LSP doesn't

**Effort:** 0 (already done)

**Risk:** Medium (UX footgun potential)

---

### Option 2: Remove LSP file filtering entirely

**Approach:** Remove `file_filter` from `CompiledConfig` and the
`file_filter.matches()` guard. The LSP validates whatever the editor opens.

**Pros:**

- Simpler (~55 LOC removed)
- No UX footgun
- Consistent: LSP validates what the editor sends, period

**Cons:**

- With `strict: true`, users may get `no-schema` on files they didn't intend to
  validate
- Removes a test case

**Effort:** 30 minutes

**Risk:** Low

---

### Option 3: Only gate strict-mode diagnostics, not all validation

**Approach:** Keep `file_filter` but only use it to suppress `no-schema`
strict-mode diagnostics. Files outside the filter still get schema validation if
they have a `$schema` or schema mapping.

**Pros:**

- Targeted: prevents spurious `no-schema` without suppressing real schema
  diagnostics
- Less surprising than gating all validation

**Cons:**

- More complex conditional logic
- Partial use of the filter may confuse maintainers

**Effort:** 45 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**

- `src/lsp.rs:24` — `file_filter` field in `CompiledConfig`
- `src/lsp.rs:420-432` — `CompiledFileFilter::compile` call and error handling
- `src/lsp.rs:459-465` — `file_filter.matches()` guard
- `tests/lsp_diagnostics.rs:237-268` —
  `strict_mode_non_matching_file_no_diagnostic` test

## Resources

- **PR:** #19
- **Issue:** #16
- **Plan:** `docs/plans/2026-02-28-feat-config-strict-mode-plan.md`

## Acceptance Criteria

- [ ] Decision made on which option to pursue
- [ ] If changing behavior, tests updated to match
- [ ] Behavior documented in help text or README

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (review agents)

**Actions:**

- code-simplicity-reviewer flagged as primary YAGNI violation
- architecture-strategist noted CLI/LSP behavioral asymmetry
- Noted this was a deliberate design decision during planning phase

**Learnings:**

- The `files` glob has different semantics in CLI (discovery) vs LSP (filtering)
  contexts
- This is a design/UX decision, not a bug
