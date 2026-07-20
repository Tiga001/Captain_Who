---
name: documents
description: Create, edit, inspect, render, and validate Microsoft Word-compatible .docx documents with the native office_document tool. Use for professional document authoring, formatting, tables, page layout, headers and footers, or other Word document work.
---

# Documents

Use `office_document` for Word document work. Do not invoke OfficeCLI through a shell command.

## Workflow

1. Call the tool's `status` operation before the first document operation in a run. Use `help` when the required operation or parameters are uncertain.
2. Inspect an existing document before editing it. Preserve unrelated content, styles, sections, and package parts.
3. Make the smallest structured change that satisfies the request. Keep every input and output inside the active workspace.
4. Render and validate the finished document when layout matters. Inspect the rendered result, not only the command result.
5. Report the output path and the checks actually performed.

If `status` reports that the Office engine is unavailable, return that error faithfully and explain which capability is missing. Never claim that a file was created, edited, rendered, or validated without a successful tool result and output verification.

Read [references/workflows.md](references/workflows.md) for detailed authoring, editing, and verification guidance when performing a document task.
