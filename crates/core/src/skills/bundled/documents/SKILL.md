---
name: documents
description: Create, edit, inspect, render, and validate Microsoft Word-compatible .docx documents. Use for professional document authoring, formatting, tables, images, page layout, headers and footers, repeated or data-driven generation, and any other Word document task.
---

# Documents

Use one of two supported paths:

- Prefer the flat semantic `office_document` tool for bounded, high-frequency work such as creating a document, adding text, images, tables, headers or footers, replacing or formatting text, inspecting structure, rendering, and validating.
- Use one saved Managed Builder for complex layouts, repeated sections, bulk generation, advanced package features, or any capability the native tool reports as unsupported.
- Combine the paths when useful: build with a script, then inspect, render, and validate with the native tool.

Never expose OfficeCLI arguments, DOM paths, executable paths, runtime versions, or package versions to the model-facing call. Every native call uses flat top-level semantic fields plus a required `reason`; never wrap it in `request`. Keep `reason` to one non-empty user-facing sentence of at most 240 characters. When the native tool returns `capabilityNotSupported` with `recovery=useManagedScript`, switch once to the Builder path instead of guessing low-level fields or repeating the failed call.

## Managed Builder

Use `skills_list_resources` to locate `templates/builder.py`, then materialize it once with `skills_materialize_resource` into a new workspace path. Patch and rerun that same builder; do not create a trail of replacement scripts.

Execute that materialized file with `run_command` and a direct logical `python <builder>.py --output <file.docx>` command. Omit `runtimeProfile` and `observe`: the host verifies this run's materialization receipt, derives the `documents` profile from the static Office output, binds the pinned runtime, and observes that output automatically. Never use system Python, `pip`, `npm`, inline code, heredocs, or shell redirection.

Bind every non-workspace input through `run_command.inputs`:

```json
{
  "mountPath": "images/campus.jpg",
  "source": {
    "type": "attachment",
    "readPath": "@attachments/<attachment-id>/campus.jpg"
  }
}
```

Use the exact attachment `readPath` returned by `attachments_list`. Use `type="workspace"`, `external`, `generated_artifact`, or `skill_resource` for those corresponding sources. Scripts read only the host-mounted path below `MYCOPILOT_INPUT_ROOT`; never pass or open an `@attachments` or `skill://` URI directly.

Every Builder command must declare its generated document with exactly one static `--output` argument. Inspect the backend-owned `artifactObservation` even after failure, timeout, or cancellation. Never blindly rerun a command that may have changed files.

## Completion gate

1. Inspect the source before an edit and prefer a distinct output unless the user requested in-place editing.
2. Generate or edit the document.
3. Confirm the expected file effect in the native result or `artifactObservation`.
4. Inspect document structure and validate the final package.
5. Render all relevant pages in one contact-sheet request. On success, pass the exact returned `outputs[].readPath` as `read_image.path`; never infer a path from the request, `argv`, `stdout`, or a file search. Visually inspect every page, patch the same Builder or semantic request when needed, then render again.
6. Report only the file effects and checks that actually succeeded. Preserve structured errors and disclose unavailable visual verification.

Read [references/workflows.md](references/workflows.md) for exact semantic and Builder examples, input binding, verification, and Word-specific quality checks.
