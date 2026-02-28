---
status: pending
priority: p2
issue_id: "001"
tags: [lsp, schema, cache]
dependencies: []
---

# Add SchemaCache::evict() for local schema file changes

## Problem Statement

`SchemaCache` uses a `HashMap<SchemaSource, Arc<SchemaSlot>>` where each slot is
a `OnceLock<SlotResult>` — once compiled, a slot is never replaced. This means
edits to local schema files (`.json`, `.schema.json`, etc.) are not picked up by
the LSP until the server is restarted. The PR description already documents this
as a known Phase 1 limitation.

## Findings

- `SchemaCache` (`src/schema.rs:430`) stores compiled validators in `OnceLock`s
  that cannot be reset once initialised.
- The `slots` field is a `Mutex<HashMap<SchemaSource, Arc<SchemaSlot>>>` —
  dropping the `Arc<SchemaSlot>` for a key and inserting a fresh one is
  sufficient to force recompilation on the next call to `get_or_compile`.
- `lsp.rs` already does exactly this pattern for `config_cache` in
  `did_change_watched_files` (`src/lsp.rs:334`).
- The `SchemaSource::File` variant carries an absolute `PathBuf`, making it a
  clean eviction key.
- URL-based schemas are not affected (they have their own HTTP disk cache with
  TTL).

## Proposed Solutions

### Option 1: `evict(source: &SchemaSource)` method

Add a targeted `SchemaCache::evict` method that removes one slot by key.

**Approach:** `self.slots.lock().unwrap().remove(source);`

**Pros:**

- Minimal API surface
- O(1) eviction cost
- Callers decide what to evict

**Cons:**

- Callers must know the exact `SchemaSource` key

**Effort:** 30 minutes

**Risk:** Low

---

### Option 2: `evict_file(path: &Path)` method

Same as Option 1 but takes a `&Path` and removes the matching
`SchemaSource::File` entry.

**Approach:** Remove entries where `SchemaSource::File(p) && p == path`.

**Pros:**

- Matches the shape of data the file-watcher will supply

**Cons:**

- Slightly more iteration (scan map), still O(n) but n is typically small

**Effort:** 30 minutes

**Risk:** Low

---

### Option 3: `evict_all()` (nuclear option)

Clear the entire in-memory compiled-schema cache.

**Pros:**

- Zero plumbing — just `self.slots.lock().unwrap().clear()`
- Safe: worst case is recompilation of all schemas on next validation

**Cons:**

- Recompiles URL schemas unnecessarily (they're still valid)

**Effort:** 15 minutes

**Risk:** Low

## Recommended Action

To be filled during triage.

## Technical Details

**Affected files:**

- `src/schema.rs:430` — `SchemaCache` struct and `get_or_compile`
- `src/lsp.rs:334` — `did_change_watched_files`, where eviction call will be
  added

**Related components:**

- Issue #002 (file watcher for local schemas) blocks the caller side of this

## Resources

- **PR:** #10
- **Known limitation:** PR description "Local schema file changes require
  restarting `jvl lsp`"

## Acceptance Criteria

- [ ] `SchemaCache` exposes an eviction method
- [ ] `did_change_watched_files` calls evict for changed local schema paths (or
      wired via #002)
- [ ] Existing tests pass
- [ ] New test: editing a local schema file triggers recompilation on next
      validation

## Work Log

### 2026-02-28 - Initial capture

**By:** Claude Code

**Actions:**

- Traced `SchemaCache` slot lifecycle in `src/schema.rs`
- Confirmed `OnceLock` cannot be reset; eviction requires map entry removal
- Identified `did_change_watched_files` as the right call site
- Drafted three options

**Learnings:**

- `config_cache` already uses the remove-and-reinsert eviction pattern — same
  approach applies here
