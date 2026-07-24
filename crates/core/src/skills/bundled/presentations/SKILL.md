---
name: presentations
description: Create, edit, inspect, render, and validate Microsoft PowerPoint-compatible .pptx presentations. Use for complete slide decks, layouts, themes, text, images, tables, charts, shapes, speaker content, repeated or data-driven generation, and any other presentation task.
---

# Presentations

Use one of two supported paths:

- Prefer the flat semantic `office_presentation` tool for bounded, high-frequency work such as creating a deck, adding, moving or removing slides, adding text, images, tables, charts, shapes or footers, inspecting, rendering, and validating.
- Use one saved Managed Builder for complete visual narratives, repeated layouts, coordinated themes, complex diagrams, template population, bulk generation, or any capability the native tool reports as unsupported.
- Combine the paths when useful: build with a script, then inspect, render, and validate with the native tool.

Never expose OfficeCLI arguments, slide DOM paths, executable paths, runtime versions, or package versions to the model-facing call. Every native call uses flat top-level semantic fields plus a required `reason`; never wrap it in `request`. Keep `reason` to one non-empty user-facing sentence of at most 240 characters. When the native tool returns `capabilityNotSupported` with `recovery=useManagedScript`, switch once to the Builder path instead of guessing low-level fields or repeating the failed call.

## Managed Builder

Use `skills_list_resources` to locate `templates/builder.mjs`, then materialize it once with `skills_materialize_resource` into a new workspace path. Patch and rerun that same builder; do not create a trail of replacement scripts.

Execute that materialized file with `run_command` and a direct logical `node <builder>.mjs --output <file.pptx>` command. Omit `runtimeProfile` and `observe`: the host verifies this run's materialization receipt, derives the `presentations` profile from the static Office output, binds the pinned runtime, and observes that output automatically. Never use system Node.js, `pip`, `npm`, inline code, heredocs, or shell redirection.

Bind templates, data, images, attachments, and earlier generated files through `run_command.inputs`:

```json
{
  "mountPath": "media/hero.png",
  "source": {
    "type": "generated_artifact",
    "uri": "image-artifact://sha256/<exact-digest>",
    "path": "/absolute/saved/path/to/hero.png"
  }
}
```

Use the exact attachment `readPath` returned by `attachments_list`. Use `type="workspace"`, `external`, `generated_artifact`, or `skill_resource` for those corresponding sources. Scripts read only the host-mounted path below `MYCOPILOT_INPUT_ROOT`; never pass or open an `@attachments` or `skill://` URI directly.

Every Builder command must declare its generated presentation with exactly one static `--output` argument. Inspect the backend-owned `artifactObservation` even after failure, timeout, or cancellation. Never blindly rerun a command that may have changed files.

## Completion gate

1. Inspect an existing deck before editing it and prefer a distinct output unless the user requested in-place editing.
2. Establish the audience, slide count, narrative, aspect ratio, and visual direction before building.
3. Confirm the expected file effect in the native result or `artifactObservation`.
4. Inspect slide order and content, then validate the final package.
5. Render all changed slides in one contact sheet. On success, pass the exact returned `outputs[].source` to `read_image`; never infer a path from the request, `argv`, `stdout`, or a file search. Visually inspect every slide for overflow, overlap, broken media, alignment, and contrast, patch the same Builder or semantic request when needed, then render again.
6. Report only the file effects and checks that actually succeeded. Preserve structured errors and disclose unavailable visual verification.

Read [references/workflows.md](references/workflows.md) for exact semantic and Builder examples, input binding, verification, and presentation-specific quality checks.
