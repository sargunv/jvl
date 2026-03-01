---
status: pending
priority: p3
issue_id: "005"
tags: [rust, cli, documentation]
dependencies: []
---

# Update --strict CLI help text to mention config option

## Problem Statement

The CLI `--strict` flag help text (`src/main.rs:108`) says "Error if any file
has no resolvable schema" but doesn't mention that `strict: true` in `jvl.json`
achieves the same thing persistently. Users won't discover the config option
from `--help`.

## Findings

- `src/main.rs:108` — current help:
  `"Error if any file has no resolvable schema"`
- Found by: architecture-strategist

## Proposed Solutions

### Option 1: Update help text

**Approach:** Change to:
`"Error if any file has no resolvable schema (also settable via strict: true in jvl.json)"`

**Effort:** 5 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Acceptance Criteria

- [ ] `--strict` help text mentions `jvl.json` config option

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (architecture-strategist)
