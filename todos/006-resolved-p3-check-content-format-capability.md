---
status: resolved
priority: p3
issue_id: "006"
tags: [code-review, architecture, lsp]
dependencies: []
---

# Check contentFormat client capability for hover response

## Problem Statement

The hover response hardcodes `MarkupKind::Markdown` without checking
`textDocument.hover.contentFormat` from the client capabilities. Per LSP spec,
servers should respect the client's supported content formats. Most modern
editors support Markdown, but spec-strict or programmatic LSP clients may only
understand PlainText.

## Findings

- `src/lsp.rs` line 511: hardcodes `kind: MarkupKind::Markdown`
- LSP spec says servers should check `textDocument.hover.contentFormat` from
  InitializeParams
- Existing `initialize` handler already parses `params.capabilities` for
  position encoding
- Found by: Agent-Native Reviewer

## Proposed Solutions

### Option 1: Store supported content formats during initialize

**Approach:** During `initialize`, check
`params.capabilities.text_document.hover.content_format` and store whether
Markdown is supported. In the hover handler, use PlainText fallback if Markdown
is not supported.

**Effort:** 20 minutes **Risk:** Low

## Recommended Action

## Acceptance Criteria

- [ ] Hover response respects client's content_format capability
- [ ] Falls back to PlainText when Markdown is not supported
- [ ] All existing tests still pass

## Work Log

### 2026-02-28 - Initial Discovery

**By:** Claude Code (Code Review)

**Actions:**

- Identified hardcoded Markdown content type in hover response
- Proposed checking client capability per LSP spec
