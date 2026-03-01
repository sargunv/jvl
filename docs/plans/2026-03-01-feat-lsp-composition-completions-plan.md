---
title: "feat: Support oneOf/anyOf/allOf in LSP completions"
type: feat
status: completed
date: 2026-03-01
origin: docs/brainstorms/2026-03-01-lsp-value-completions-brainstorm.md
deepened: 2026-03-01
---

# feat: Support oneOf/anyOf/allOf in LSP completions

## Enhancement Summary

**Deepened on:** 2026-03-01 **Research agents used:**
pattern-recognition-specialist, performance-oracle, code-simplicity-reviewer,
architecture-strategist, best-practices-researcher, Context7

### Key Improvements from Research

1. **Critical fix**: Do NOT derive `Eq` on `ValueSuggestion` —
   `serde_json::Value` contains `f64` which does not implement `Eq`. Use
   `PartialEq` only.
2. **Architecture change**: Do NOT modify `resolve_subschema()` for
   `oneOf`/`anyOf` — it would change behavior for hover and property completion
   callers. Handle composition walking exclusively in collection functions.
3. **Simplification**: Normalize `type` field into a `Vec<&str>` to eliminate
   duplicated match arms for string vs array.
4. **Validated by production servers**: vscode-json-languageservice,
   yaml-language-server, and taplo all use union semantics for all composition
   keywords — our approach matches industry practice.

### New Considerations Discovered

- `collect_properties_from` line 818-821 also reads `type` as string-only —
  consider fixing for display (e.g., "string | null")
- Boolean JSON schemas (`true`/`false` as schema values) are harmless no-ops in
  the composition walker — no special handling needed

## Overview

Extend LSP completions to walk `oneOf`, `anyOf`, and `allOf` composition
keywords for both property key and property value completions. Also handle
`type` as an array (e.g., `["string", "null"]`).

Currently, `collect_properties_from()` only recurses into `allOf` branches, and
`collect_values()` only inspects the top-level resolved subschema. Schemas using
`oneOf`/`anyOf` produce no completions for properties or values defined within
those branches.

## Problem Statement / Motivation

Many real-world JSON schemas use composition keywords. For example, VS Code's
`settings.json` schema and `tsconfig.json` schema heavily use `oneOf`/`anyOf`
for property definitions and value constraints. Without composition support,
completions silently produce no results for these schemas.

## Proposed Solution

Three coordinated changes:

1. **Extend `collect_properties_from()`** to recurse into `oneOf`/`anyOf`
   branches (it already handles `allOf`).

2. **Restructure `collect_values()`** with two helpers:
   `collect_values_at_property()` walks the parent schema's composition branches
   to find all definitions of the target property, and `collect_values_from()`
   extracts const/enum/type suggestions from a single schema node and recurses
   into its composition branches.

3. **Handle `type` as array** in `collect_values_from()` — normalize to a
   `Vec<&str>` and emit `Boolean`/`Null` suggestions for matching entries.

**Key design decisions** (see brainstorm:
`docs/brainstorms/2026-03-01-lsp-value-completions-brainstorm.md`):

- **Union semantics** for all composition keywords (`allOf`, `oneOf`, `anyOf`) —
  pragmatic, matches how `collect_properties_from` already works for `allOf`.
  Validated by vscode-json-languageservice, yaml-language-server, and taplo
  which all use union semantics.
- **Local early returns** — `const`/`enum` early returns apply per-branch, not
  globally. Each composition branch contributes suggestions independently.
- **Deduplication by value** — deduplicate `ValueSuggestion` entries using
  `PartialEq` via `Vec::contains()`. Acceptable for the small cardinality of
  typical suggestion lists (< 50 items).
- **Depth + cycle guards** — reuse `MAX_SCHEMA_DEPTH` (32) and fresh `visited`
  sets per `follow_ref` call, matching the existing pattern in
  `collect_properties_from()`. This is actually more defensive than production
  LSP servers (vscode-json and yaml-language-server have no explicit depth
  limits).
- **Do NOT modify `resolve_subschema()`** — composition walking happens
  exclusively in the collection functions to avoid changing behavior for hover
  and other callers.

## Technical Approach

### Phase 1: Extend `collect_properties_from()` for `oneOf`/`anyOf`

**File: `src/schema.rs`, lines 781-838**

Add `oneOf` and `anyOf` recursion after the existing `allOf` block (line
833-837):

```rust
// Existing allOf handling (lines 833-837):
if let Some(all_of) = schema.get("allOf").and_then(|v| v.as_array()) {
    for branch in all_of {
        collect_properties_from(root, branch, props, seen, depth + 1);
    }
}

// NEW: Walk oneOf and anyOf branches identically.
for keyword in &["oneOf", "anyOf"] {
    if let Some(branches) = schema.get(*keyword).and_then(|v| v.as_array()) {
        for branch in branches {
            collect_properties_from(root, branch, props, seen, depth + 1);
        }
    }
}
```

The existing `seen: HashSet<String>` deduplication handles property name
collisions across branches. First-seen definition wins, matching the pattern
used by vscode-json-languageservice.

#### Research Insights

**Best Practices (from production servers):**

- vscode-json-languageservice uses `Map<string, CompletionItem>` keyed by label
  for property dedup — our `HashSet<String>` is equivalent
- yaml-language-server attempts to merge documentation when duplicate properties
  come from different branches — not needed for MVP but could be a follow-up

**Edge Cases:**

- Boolean schemas (`true`/`false` as schema values) in composition branches:
  `collect_properties_from` calls `.get("properties")` which returns `None` on a
  `Value::Bool` — harmless no-op, no special handling needed
- Empty composition arrays (`oneOf: []`): the `for branch in branches` loop
  simply doesn't execute — correct behavior

### Phase 2: Restructure `collect_values()` with recursive helpers

**File: `src/schema.rs`**

2a. **Add `PartialEq` derive to `ValueSuggestion`** (line 752):

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ValueSuggestion { ... }
```

**Important**: Do NOT derive `Eq`. `serde_json::Value` contains `f64` which does
not implement `Eq`. `Vec::contains()` only requires `PartialEq`, which is
sufficient.

2b. **Restructure `collect_values()` to walk composition at parent level**:

The current `collect_values()` resolves the full pointer (parent +
property_name) to a single subschema. Instead, it should resolve to the
**parent** schema, then walk that parent's `properties` and composition branches
to find all definitions of the target property. This is critical because
properties defined inside `oneOf`/`anyOf` branches at the parent level would be
missed by a single `resolve_subschema_at_pointer` call.

```rust
pub fn collect_values(
    root: &serde_json::Value,
    pointer: &[String],
    property_name: &str,
) -> Vec<ValueSuggestion> {
    let mut suggestions = Vec::new();

    // Resolve to the parent schema (the containing object).
    let parent = if pointer.is_empty() {
        let mut visited = HashSet::new();
        match follow_ref(root, root, &mut visited) {
            Some(s) => s,
            None => return suggestions,
        }
    } else {
        match resolve_subschema_at_pointer(root, pointer) {
            Some(s) => s,
            None => return suggestions,
        }
    };

    // Collect values from this parent's definition of the property.
    collect_values_at_property(root, parent, property_name, &mut suggestions, 0);
    suggestions
}
```

2c. **`collect_values_at_property()` — parent-level composition walker**:

This function mirrors the pattern of `collect_properties_from()`: it walks the
parent schema's `properties` and composition branches to find all definitions of
the target property name. For each definition found, it delegates to
`collect_values_from()` for leaf-level value extraction.

```rust
fn collect_values_at_property(
    root: &serde_json::Value,
    parent_schema: &serde_json::Value,
    property_name: &str,
    suggestions: &mut Vec<ValueSuggestion>,
    depth: usize,
) {
    if depth > MAX_SCHEMA_DEPTH {
        return;
    }

    // Follow $ref on the parent.
    let mut visited = HashSet::new();
    let Some(parent_schema) = follow_ref(root, parent_schema, &mut visited) else {
        return;
    };

    // Check properties.<property_name> on this schema node.
    if let Some(prop_schema) = parent_schema
        .get("properties")
        .and_then(|p| p.get(property_name))
    {
        collect_values_from(root, prop_schema, suggestions, depth + 1);
    }

    // Walk composition branches at the parent level.
    for keyword in &["allOf", "oneOf", "anyOf"] {
        if let Some(branches) = parent_schema.get(*keyword).and_then(|v| v.as_array()) {
            for branch in branches {
                collect_values_at_property(
                    root, branch, property_name, suggestions, depth + 1,
                );
            }
        }
    }
}
```

2d. **`collect_values_from()` — leaf-level value extractor**:

This function extracts const/enum/type suggestions from a single schema node,
then recurses into composition branches at the leaf level. It follows the
`collect_properties_from()` pattern: `root` threaded through, mutable
accumulator, depth guard.

```rust
fn collect_values_from(
    root: &serde_json::Value,
    schema: &serde_json::Value,
    suggestions: &mut Vec<ValueSuggestion>,
    depth: usize,
) {
    if depth > MAX_SCHEMA_DEPTH {
        return;
    }

    // Follow $ref.
    let mut visited = HashSet::new();
    let Some(schema) = follow_ref(root, schema, &mut visited) else {
        return;
    };

    // Check for const (local early return — only this branch).
    if let Some(const_val) = schema.get("const") {
        let suggestion = ValueSuggestion::Const(const_val.clone());
        if !suggestions.contains(&suggestion) {
            suggestions.push(suggestion);
        }
        return;
    }

    // Check for enum (local early return — only this branch).
    if let Some(enum_vals) = schema.get("enum").and_then(|v| v.as_array()) {
        for val in enum_vals {
            let suggestion = ValueSuggestion::Enum(val.clone());
            if !suggestions.contains(&suggestion) {
                suggestions.push(suggestion);
            }
        }
        return;
    }

    // Check type — normalize string and array forms into a uniform iterator.
    let types: Vec<&str> = match schema.get("type") {
        Some(serde_json::Value::String(s)) => vec![s.as_str()],
        Some(serde_json::Value::Array(arr)) => {
            arr.iter().filter_map(|v| v.as_str()).collect()
        }
        _ => vec![],
    };
    for t in &types {
        match *t {
            "boolean" => {
                if !suggestions.contains(&ValueSuggestion::Boolean) {
                    suggestions.push(ValueSuggestion::Boolean);
                }
            }
            "null" => {
                if !suggestions.contains(&ValueSuggestion::Null) {
                    suggestions.push(ValueSuggestion::Null);
                }
            }
            _ => {}
        }
    }

    // Recurse into composition keywords.
    for keyword in &["allOf", "oneOf", "anyOf"] {
        if let Some(branches) = schema.get(*keyword).and_then(|v| v.as_array()) {
            for branch in branches {
                collect_values_from(root, branch, suggestions, depth + 1);
            }
        }
    }
}
```

#### Research Insights

**Architecture (from architecture-strategist):**

- Do NOT modify `resolve_subschema()` for `oneOf`/`anyOf`. It has three callers
  (hover, property completion, value completion). Adding first-match
  `oneOf`/`anyOf` would silently change hover behavior and could cause property
  completion to miss branches when `oneOf` appears higher in the schema tree.
- The two-function split (`collect_values_at_property` + `collect_values_from`)
  correctly mirrors the existing `collect_properties` /
  `collect_properties_from` two-phase pattern.

**Performance (from performance-oracle):**

- `Vec::contains()` for dedup is O(n) per insertion, but suggestion lists are
  small (< 50 items). Total cost is negligible compared to schema compilation
  and I/O.
- Fresh `HashSet::new()` per `follow_ref` call is correct — Rust's
  `HashSet::new()` doesn't allocate until first insertion. Sharing visited sets
  across branches would be a correctness bug (two branches can legitimately
  reference the same `$ref`).
- Worst-case complexity with branching factor B and depth D is O(B^D), but
  `MAX_SCHEMA_DEPTH = 32` prevents infinite recursion. Real-world schemas rarely
  exceed 4-5 levels of composition nesting.

**Pattern consistency (from pattern-recognition-specialist):**

- `collect_values_from()` name follows the `{verb}_{noun}_from` convention
  matching `collect_properties_from()`
- `collect_values_at_property()` name is reasonable as the parent-level walker
  (alternative: fold inline into `collect_values()` if the function stays small)

**Simplicity (from code-simplicity-reviewer):**

- Normalizing `type` to `Vec<&str>` eliminates all code duplication between
  string and array forms
- The two-function split is justified because they operate at different levels:
  parent-level (finding the property) vs. leaf-level (extracting values)

### Phase 3: Tests

**3a. Unit tests in `src/schema.rs`** (after existing tests at ~line 1400):

| Test Name                               | Schema                                                      | Expected                         |
| --------------------------------------- | ----------------------------------------------------------- | -------------------------------- |
| `collect_values_oneof_enums`            | `{ oneOf: [{ enum: ["a","b"] }, { enum: ["c","d"] }] }`     | 4 Enum suggestions               |
| `collect_values_anyof_mixed`            | `{ anyOf: [{ enum: ["x"] }, { type: "boolean" }] }`         | Enum("x") + Boolean              |
| `collect_values_allof_union`            | `{ allOf: [{ enum: ["a","b"] }, { enum: ["b","c"] }] }`     | 3 Enum suggestions (deduped)     |
| `collect_values_type_array`             | `{ type: ["boolean", "null"] }`                             | Boolean + Null                   |
| `collect_values_type_array_string_only` | `{ type: ["string", "number"] }`                            | empty                            |
| `collect_values_nested_composition`     | `{ oneOf: [{ allOf: [{ enum: ["a"] }] }, { const: "b" }] }` | Enum("a") + Const("b")           |
| `collect_values_ref_in_composition`     | `{ oneOf: [{ $ref: "#/$defs/X" }] }` where X has enum       | enum values from X               |
| `collect_values_composition_at_parent`  | parent has `oneOf` with different property defs             | values from both branches        |
| `collect_values_dedup_across_branches`  | two branches both define `enum: ["same"]`                   | 1 suggestion, not 2              |
| `collect_properties_oneof`              | `oneOf` branches with different properties                  | all properties from all branches |
| `collect_properties_anyof`              | `anyOf` branches with different properties                  | all properties from all branches |

**3b. Integration tests in `tests/lsp_completion.rs`**:

Add new properties to `tests/fixtures/completion-schema.json`:

```json
{
  "nullable_flag": { "oneOf": [{ "type": "boolean" }, { "type": "null" }] },
  "multi_enum": {
    "anyOf": [{ "enum": ["fast", "slow"] }, { "enum": ["medium"] }]
  },
  "typed_nullable": { "type": ["string", "null"] }
}
```

Integration test cases:

| Test Name                         | Document Content                        | Expected Labels              |
| --------------------------------- | --------------------------------------- | ---------------------------- |
| `completion_oneof_values`         | `{ "nullable_flag":                     | }`                           |
| `completion_anyof_enum_values`    | `{ "multi_enum":                        | }`                           |
| `completion_type_array_values`    | `{ "typed_nullable":                    | }`                           |
| `completion_oneof_property_names` | New schema with `oneOf` at object level | Properties from all branches |

#### Research Insights

**Test Coverage (from best-practices-researcher):**

- yaml-language-server tests `oneOf` with a discriminator property — not needed
  for MVP but good future test case
- Consider a test with `allOf` that constrains without conflicting (e.g.,
  `allOf: [{ type: "string" }, { enum: ["a", "b"] }]`) — the enum should be
  returned since we use union semantics

## Acceptance Criteria

- [x] `collect_values` returns suggestions from `oneOf`/`anyOf`/`allOf` branches
- [x] `collect_values` handles `type` as array (e.g., `["boolean", "null"]`)
- [x] `collect_properties_from` walks `oneOf`/`anyOf` branches for property key
      completions
- [x] Suggestions are deduplicated across composition branches
- [x] Depth limiting prevents infinite recursion on circular schemas
- [x] All new unit tests pass
- [x] All new integration tests pass
- [x] All existing tests continue to pass

## Known Limitations (Out of Scope)

- **`if`/`then`/`else`** composition: not handled, lower priority. Architecture
  supports adding it later by extending the composition keyword list.
- **`not` keyword**: ignored (cannot produce positive suggestions)
- **`default`/`examples`**: not surfaced as completions (per brainstorm)
- **Array item completions**: deferred to follow-up (requires
  `completion_context` parser changes)
- **`$schema` URL completions**: out of scope
- **`allOf` intersection semantics**: uses union (pragmatic), may suggest values
  that fail validation when `allOf` branches have conflicting constraints. All
  production LSP servers (vscode-json, yaml, taplo) also use union.
- **`resolve_subschema()` unchanged**: hover will show first-match annotation
  when properties appear in multiple `oneOf` branches. This is acceptable and
  consistent with current hover behavior.
- **Property type display for array types**: `collect_properties_from` line
  818-821 reads `type` as string only — "string | null" display would require a
  separate fix

## Sources & References

- **Origin brainstorm:**
  [docs/brainstorms/2026-03-01-lsp-value-completions-brainstorm.md](docs/brainstorms/2026-03-01-lsp-value-completions-brainstorm.md)
  — key decisions: focus on collect_values restructuring, union semantics, defer
  array items
- Model pattern: `collect_properties_from()` at `src/schema.rs:781`
- Value collection: `collect_values()` at `src/schema.rs:856`
- Subschema resolution: `resolve_subschema()` at `src/schema.rs:897`
- LSP completion handler: `src/lsp.rs:551`
- Value item builder: `build_value_items()` at `src/lsp.rs:1031`
- Test fixtures: `tests/fixtures/completion-schema.json`
- Integration tests: `tests/lsp_completion.rs`
- JSON Schema composition spec:
  https://json-schema.org/understanding-json-schema/reference/combining
- Production LSP patterns: vscode-json-languageservice, yaml-language-server,
  taplo
