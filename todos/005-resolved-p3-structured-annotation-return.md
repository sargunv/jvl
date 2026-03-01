---
status: resolved
priority: p3
issue_id: "005"
tags: [code-review, architecture, agent-native]
dependencies: []
---

# Split lookup_hover_content into structured annotation + formatting layers

## Problem Statement

`lookup_hover_content` returns pre-formatted Markdown (`Option<String>`). A
programmatic consumer that wants the raw `title` and `description` separately
must parse the Markdown back apart. Splitting into a structured return type plus
a formatting layer would improve programmatic reuse.

## Findings

- `src/schema.rs` `lookup_hover_content` returns `Option<String>` with Markdown
  formatting baked in
- `format_hover` combines title/description into `**title**\n\ndescription`
  Markdown
- A `SchemaAnnotation { title: Option<String>, description: Option<String> }`
  intermediate type would decouple data from presentation
- Found by: Agent-Native Reviewer

## Proposed Solutions

### Option 1: Return struct, format in LSP layer

**Approach:** `lookup_schema_annotation` returns `Option<SchemaAnnotation>`,
hover handler calls `SchemaAnnotation::to_markdown()`.

**Effort:** 20 minutes **Risk:** Low

## Recommended Action

## Acceptance Criteria

- [ ] Structured annotation type available for programmatic consumers
- [ ] LSP hover handler formats to Markdown separately
- [ ] All existing tests still pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (Code Review)

**Actions:**

- Identified Markdown formatting mixed into data layer
- Proposed structured return type for better programmatic reuse
