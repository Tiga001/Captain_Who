---
name: documents
description: Create, edit, inspect, render, and validate Microsoft Word-compatible .docx documents. Use for professional document authoring, formatting, tables, images, page layout, headers and footers, repeated or data-driven generation, and any other Word document task.
---

# Documents

Route by intent:

- **Read:** use the flat semantic `office_document` tool to inspect, render, and validate a document.
- **Create:** use native operations for a small bounded document, or one saved Managed Builder for complex layouts, repeated sections, bulk generation, or advanced generation features.
- **Edit an existing `.docx`:** inspect first and use only supported native edit operations. The current Builder is creation-oriented, not a fidelity-preserving existing-document Editor. If the requested edit is outside the native surface, preserve the source and report it as unsupported until a fixed Editor exists; never rebuild the document to imitate an edit.

Combine the paths only at their intended boundaries: native inspect/render/validate around one Builder creation, or native inspect/edit/verify for a bounded existing-document change.
Each native mutation is an independent atomic file transaction, not a multi-operation batch. After `removeBlock` or `moveBlock`, re-inspect before using another `blockIndex` because indices may shift.

Never expose OfficeCLI arguments, DOM paths, executable paths, runtime versions, or package versions to the model-facing call. Every native call uses flat top-level semantic fields plus a required `reason`; never wrap it in `request`. Keep `reason` to one non-empty user-facing sentence of at most 240 characters. For a new document, `capabilityNotSupported` with `recovery=useManagedScript` may route to the Builder. For an existing document, do not treat that recovery hint as permission to rebuild it; fail closed when no native operation can preserve fidelity.

## Managed Builder

Before materializing, choose one dedicated workspace-relative script directory. Reuse it if it is already a plain directory; otherwise create it first with one separate idempotent `mkdir -p <script-directory>` `run_command`. The directory name is not fixed. Materialize `templates/builder.py` once into a new path inside it, then patch and rerun that same Builder.

The Host automatically runs an isolated Python syntax preflight before every Builder execution. A script change invalidates the earlier result; a failed preflight forbids execution, so patch the same Builder and run it again. Do not issue a separate model-authored syntax command or invoke system Python, `python -m`, `python -c`, inline code, or shell composition for this gate.

Execute that materialized file with `run_command` and a direct logical `python <builder>.py --output <file.docx>` command. Omit `runtimeProfile` and `observe`: the host verifies this run's materialization receipt, derives the `documents` profile from the static Office output, binds the pinned runtime, and observes that output automatically. Never use system Python, `pip`, `npm`, inline code, heredocs, or shell redirection.

Bind every non-workspace input through `run_command.inputs`:

```json
{
  "mountPath": "images/campus.jpg",
  "path": "@attachments/<attachment-id>/campus.jpg"
}
```

Use the exact path returned by the producing tool or supplied by the user. The Host automatically recognizes workspace, absolute/system, `@attachments/...`, `image-artifact://...`, and revision-bound `skill://...` paths. `mountPath` is optional and defaults to the source filename. Scripts read only the host-mounted path below `MYCOPILOT_INPUT_ROOT`; never pass or open an `@attachments` or `skill://` URI directly.

Every Builder command must declare its generated document with exactly one static `--output` argument. Inspect the backend-owned `artifactObservation` even after failure, timeout, or cancellation. Never blindly rerun a command that may have changed files.

After the final checks, delete the exact Builder and task-created temporary files. If this task created the script directory and it is then empty, remove it with `rmdir`. Preserve pre-existing directories and unrelated files, never use recursive deletion, and keep the Builder only when the user explicitly asks for it.

## Completion gate

1. Inspect an existing source before a supported native edit and prefer a distinct output unless the user requested in-place editing. Never route an existing-document edit through the Builder.
2. Generate the document or apply the bounded native edit.
3. Confirm the expected file effect in the native result or `artifactObservation`.
4. Inspect the final document and validate the package. The current semantic surface does not provide an authoritative rendered page count; never infer one from `pageSelection`, a filename, or a contact sheet. When comments, tracked changes, or fields are present or required, run an explicit structural check; rendering alone does not verify them.
5. Render the whole document once for an overview, then render any known or affected page individually with one `pageOrSlide` per request. Pass only exact returned `outputs[].readPath` values to `read_image.path`, and keep a numbered ledger of the pages actually read. Unless a future trusted result explicitly supplies the final page count, disclose that exhaustive all-page coverage was unavailable rather than claiming a complete `1..N` review.
6. Any final edit invalidates the earlier structural, validation, and visual evidence. Repeat the affected checks, then report only what actually succeeded and disclose unavailable structural or visual verification.

Read [references/workflows.md](references/workflows.md) for exact semantic and Builder examples, input binding, verification, and Word-specific quality checks.
