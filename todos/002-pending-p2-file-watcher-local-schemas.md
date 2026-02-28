---
status: pending
priority: p2
issue_id: "002"
tags: [lsp, schema, file-watcher]
dependencies: ["001"]
---

# Watch local schema files for changes and re-validate on edit

## Problem Statement

The LSP already watches `**/jvl.json` for config changes (`src/lsp.rs:239`).
However, local schema files referenced by `$schema` fields or `jvl.json`
mappings are not watched. Editing a local schema file while the server is
running produces no re-validation — the compiled validator from the old schema
continues to be used until restart.

Depends on #001 (SchemaCache::evict) to actually invalidate the compiled
validator once a schema file change is detected.

## Findings

- `initialized` (`src/lsp.rs:236`) registers one `FileSystemWatcher` with glob
  `**/jvl.json`.
- The same `register_capability` call can accept multiple `FileSystemWatcher`
  entries — a second registration for schema patterns is straightforward.
- The challenge is that schema paths aren't all known at `initialized` time;
  they're discovered lazily per-document. Two strategies exist: register a broad
  glob, or track schema paths seen during validation and register them
  incrementally.
- `did_change_watched_files` (`src/lsp.rs:324`) already receives file events —
  adding schema eviction logic there requires only checking whether the changed
  path matches a known schema source.
- `SchemaSource::File` paths are absolute, so matching against watcher events is
  exact.

## Proposed Solutions

### Option 1: Register broad glob patterns at startup

Add watchers for `**/*.json` and `**/*.schema.json` (or a configurable set)
alongside `**/jvl.json` in `initialized`.

**Pros:**

- Simple, no per-document tracking needed
- Catches all local schema files immediately

**Cons:**

- Watches every `.json` file in the workspace, generating many events
- Can't distinguish schema files from data files without checking the
  `SchemaSource` set

**Effort:** 1–2 hours

**Risk:** Low (noisy events are benign — eviction of a non-cached key is a
no-op)

---

### Option 2: Track schema paths seen during validation, register incrementally

When a validation resolves a `SchemaSource::File`, record it in a `HashSet` on
`Backend`. After each validation, register new schema paths with the client as
file watchers.

**Pros:**

- Only watches paths that are actually used
- No spurious events from unrelated `.json` files

**Cons:**

- More state to manage on `Backend`
- `register_capability` is async; needs care in `spawn_blocking` callback

**Effort:** 3–4 hours

**Risk:** Medium (more moving parts, async/sync boundary)

---

### Option 3: Re-validate all open documents on any `.json` file change (no watcher)

Skip granular eviction: whenever any `.json` file changes in the workspace,
clear the entire schema cache and re-validate all open documents.

**Pros:**

- Simplest logic: piggyback on the existing `**/jvl.json` watcher expanded to
  `**/*.json`
- Correctness: always fresh

**Cons:**

- Overkill: re-validates every document when any `.json` in the workspace
  changes
- Scales poorly in large workspaces

**Effort:** 1 hour

**Risk:** Low

## Recommended Action

To be filled during triage.

## Technical Details

**Affected files:**

- `src/lsp.rs:236` — `initialized`, watcher registration
- `src/lsp.rs:324` — `did_change_watched_files`, eviction + re-validation logic
- `src/schema.rs` — `SchemaCache::evict` (prerequisite, see #001)

**Related components:**

- Issue #001 (SchemaCache::evict) — must be done first

## Resources

- **PR:** #10
- **Known limitation:** PR description "Local schema file changes require
  restarting `jvl lsp`"

## Acceptance Criteria

- [ ] Editing a local schema file triggers re-validation of all open documents
      that use it
- [ ] No spurious re-validations for unrelated file changes (if Option 2 chosen)
- [ ] Existing LSP tests pass
- [ ] New integration test: schema file change → diagnostics update

## Work Log

### 2026-02-28 - Initial capture

**By:** Claude Code

**Actions:**

- Traced `initialized` watcher registration and `did_change_watched_files`
  handler
- Identified two viable strategies (broad glob vs. incremental tracking)
- Noted async/sync boundary constraint for incremental approach

**Learnings:**

- `register_capability` can accept multiple watchers in one call — no need for
  multiple round-trips
- Blocked on #001 (eviction API) before the watcher side is useful
