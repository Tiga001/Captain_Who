---
name: documents
description: Create, edit, inspect, and render Microsoft Word-compatible .docx documents. Use for professional document authoring, existing-document changes, formatting, tables, images, page layout, headers and footers, repeated or data-driven generation, and any other Word document task.
---

# Documents

Route by intent:

- **Read or verify:** use the flat semantic `office_document` surface only for `inspect` and
  `render`.
- **Create a new `.docx`:** materialize and run the Managed Builder from `templates/builder.py`. Do not assemble a new
  document through a sequence of native write calls.
- **Edit an existing `.docx`:** inspect first, then materialize and run `templates/editor.py` with
  one mounted source and one distinct save-as output. Never rebuild the source with the Builder and
  never substitute native write calls for the Editor transaction.

Do not expose OfficeCLI arguments, DOM paths, executable paths, runtime versions, or package
versions to model-facing Office calls. Native read calls use flat top-level semantic fields and a
required non-empty user-facing `reason` of at most 240 characters; never wrap them in `request`.

## Managed Python scripts

Choose one dedicated workspace-relative script directory for the task. Reuse it when it already
exists as a plain directory; otherwise create it first with one separate idempotent
`mkdir -p <script-directory>` `run_command`. The directory name is not fixed.
Reuse this directory for the temporary visual-QA PDF; never place that PDF in the workspace's
top-level `outputs/` directory. Its `office_document.render` `outputPath` must be
workspace-relative; never pass an absolute path.

Locate the exact revision-bound template URI with `skills_list_resources`, then materialize the
selected Builder or Editor once into a new path. Patch and rerun that same file after corrections;
do not rematerialize over a modified script or create a new script per retry.

The Host automatically performs an isolated Python syntax preflight on every frozen script
revision before execution. A patch invalidates the prior result. On failure, patch the same script
and submit its normal command again. Do not issue a separate model-authored syntax command or use
system Python, `python -m`, `python -c`, inline code, heredocs, shell composition, `pip`, or `npm`.

Run the materialized file with a direct logical command:

- Builder: `python <builder>.py --output <new.docx>`
- Editor: `python <editor>.py --source <mounted-source.docx> --output <edited.docx>`

Prefer a workspace-root final output such as `report.docx`. If the output is nested, its parent
directory must already exist before `run_command`; the Host checks it before the script starts, so
the script cannot create it in time.

Every run declares exactly one static `--output`. An Editor also declares exactly one static
`--source`, equal to one `run_command.inputs[].mountPath`, and a distinct `.docx` output. Omit
`runtimeProfile` and `observe`: the Host verifies the run-scoped materialization receipt, derives
the pinned `documents` runtime, freezes the script and inputs, runs preflight, directs Office output
through a private candidate, runs the pinned OfficeCLI schema gate, publishes atomically, and binds
observation. The fixed script saves once to the Host-provided output path; do not add another
temporary file, candidate reopen, `os.replace`, or model-side validation layer.

Bind every non-workspace input through `run_command.inputs`. Scripts resolve only declared logical
mount names below `MYCOPILOT_INPUT_ROOT`; never pass or open `@attachments`, `skill://`, artifact
URIs, attachment-library paths, or Host-private paths directly.

The Word Editor executes normal Python with the pinned `python-docx` runtime. Its marked edit region
may use functions, pinned imports, loops, conditions, and data transformations; it is not an AST or
JSON operation DSL. This freedom remains inside the existing managed-command permission and
approval boundary and does not make unrelated Python side effects transactional. Read
[references/editing-existing.md](references/editing-existing.md) before editing for the exact
workflow, preservation rules, and unsupported OOXML boundaries.

Inspect `artifactObservation` even after failure, timeout, or cancellation. When `run_command`
returns `status: "running"`, follow its `continueWith` receipt and wait with `command_session` for
the terminal result. A process exit, filename, preliminary observation, or running receipt is not
publication evidence.

If an unexpected limitation makes the fixed Builder, Editor, or standard workflow unable to
create or edit the `.docx`, use one task-scoped self-authored `.py` file in the same temporary
directory as the exceptional fallback. Run it with a direct `python <script>.py ...` `run_command`;
set `runtimeProfile: "documents"`; normal Python, `python-docx`, and feature-specific OOXML are
available. Bind every non-workspace input through `run_command.inputs` and save an edited source to
a distinct `.docx`. A self-authored script has no template materialization receipt, so it does not
receive the template path's Host-private candidate transaction: make the script save to a
task-temporary `.docx`, reopen and sanity-check it, then atomically replace the declared output.
After terminal success, require the expected `artifactObservation`, inspect the output, and run the
normal PDF visual QA before cleanup. Do not use this fallback merely to replace a working template
path, and never use it to replace the managed DOCX-to-PDF visual-QA conversion. ReportLab
reconstruction, page-image stitching, and other model-authored DOCX-to-PDF converters are
forbidden as visual evidence.

After final checks, delete the exact Builder, Editor or fallback script, temporary QA PDF, any
workspace page images, and other task-created workspace files. PDF Skill page images returned as
managed Artifact `readPath` values are not workspace files: keep those paths until they have been
read, then let the Host clean its managed Run workspace instead of searching for or deleting them.
If this task created the script directory and it is then empty, remove it with `rmdir`. Preserve
pre-existing directories and unrelated files; never use recursive deletion. Keep scripts only when
the user explicitly asks for them. The QA PDF is temporary evidence, not a final artifact; do not
present it as a delivery card unless the user explicitly requested a PDF.

## Completion gate

1. Inspect an existing source before editing and establish a structural and visual baseline for
   the affected content.
2. Create with the Builder or edit with the Editor. Confirm terminal success and an
   `artifactObservation` file effect for the declared output.
3. The successful terminal result proves that the Host's pinned OfficeCLI schema gate accepted the
   private candidate before publication. Do not call native `validate`. Inspect the final document.
   For comments, tracked changes, fields, content controls, or other fidelity-sensitive parts, run
   an explicit structural check; rendering is not structural evidence.
4. Convert the final `.docx` once to a task-temporary PDF with `office_document.render`, supplying
   `outputFormat: "pdf"` and a `.pdf` `outputPath` inside the existing task script directory, never
   top-level `outputs/`. Require one returned PDF output containing `outputs[].readPath`,
   `outputs[].pageCount`, `outputs[].sourceSha256`, and `outputs[].rendererRevision`; use only that
   exact `readPath`, then activate and follow the PDF Skill. Pass the returned `readPath` unchanged
   as the PDF source for `pdfinfo` and `pdftoppm`; do not add `run_command.inputs` or rewrite it
   beneath `MYCOPILOT_INPUT_ROOT`.
   Bind the evidence ledger to `sourceSha256` and invalidate it after any DOCX change. Treat
   `pdfinfo` as the authoritative page count `N`, require it to agree with the returned PDF output's
   `pageCount`, render pages `1..N` with `pdftoppm` in contiguous batches of at most 32 pages,
   consume every returned page image with `read_image.path` before cleaning that batch, and keep one
   verdict per page. This is a Skill workflow instruction, not a runtime-enforced handoff.
5. Compare requested changes and unrelated content with the baseline. Any final edit invalidates
   the prior PDF, page count, page images, ledger, inspection, Host publication, structural, and
   visual evidence; regenerate them from the new final file.
6. Report only checks that actually succeeded and disclose unsupported fidelity or unavailable
   visual coverage. For documents containing `PAGE` or `NUMPAGES`, verify in the rendered PDF that
   each field is visible and correct for the document's page-numbering rules and that `NUMPAGES`
   agrees with authoritative `N`.

Read [references/workflows.md](references/workflows.md) for creation, input binding, observation,
PDF handoff, render consumption, exceptional fallback, cleanup, and quality checks. Read
[references/editing-existing.md](references/editing-existing.md) for every existing-document edit.
