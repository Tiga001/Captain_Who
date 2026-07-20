---
name: presentations
description: Create, edit, inspect, render, and validate Microsoft PowerPoint-compatible .pptx presentations with the native office_presentation tool. Use for slide decks, layouts, themes, text, shapes, tables, charts, images, speaker notes, or other presentation work.
---

# Presentations

Use `office_presentation` for PowerPoint presentation work. Do not invoke OfficeCLI through a shell command.

## Workflow

1. Call the tool's `status` operation before the first presentation operation in a run. Use `help` when the required operation or parameters are uncertain.
2. Inspect an existing deck's structure, layouts, theme, and relevant slides before editing it.
3. Make the smallest structured change that satisfies the request. Keep every input and output inside the active workspace.
4. Render every changed slide and inspect the images for overflow, collisions, illegible text, broken media, and inconsistent composition.
5. Validate the finished deck, then report the output path and the checks actually performed.

If `status` reports that the Office engine is unavailable, return that error faithfully and explain which capability is missing. Never claim that a deck was created, edited, rendered, or validated without a successful tool result and output verification.

Read [references/workflows.md](references/workflows.md) for detailed deck planning, editing, rendering, and verification guidance when performing a presentation task.
