---
name: documents
description: Create, edit, inspect, render, and validate Microsoft Word-compatible .docx documents. Use for professional document authoring, existing-document changes, formatting, tables, images, page layout, headers and footers, repeated or data-driven generation, and any other Word document task.
---

# Documents

Route by intent:

- **Read or verify:** use the flat semantic `office_document` surface for `status`, `inspect`,
  `validate`, and `render`.
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

Every run declares exactly one static `--output`. An Editor also declares exactly one static
`--source`, equal to one `run_command.inputs[].mountPath`, and a distinct `.docx` output. Omit
`runtimeProfile` and `observe`: the Host verifies the run-scoped materialization receipt, derives
the pinned `documents` runtime, freezes the script and inputs, runs preflight, directs Office output
through a private candidate, validates it, publishes atomically, and binds observation.

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

After final checks, delete the exact Builder or Editor and task-created temporary files. If this
task created the script directory and it is then empty, remove it with `rmdir`. Preserve
pre-existing directories and unrelated files; never use recursive deletion. Keep scripts only when
the user explicitly asks for them.

## Completion gate

1. Inspect an existing source before editing and establish a structural and visual baseline for
   the affected content.
2. Create with the Builder or edit with the Editor. Confirm terminal success and an
   `artifactObservation` file effect for the declared output.
3. Inspect the final document and validate the package. For comments, tracked changes, fields,
   content controls, or other fidelity-sensitive parts, run an explicit structural check;
   rendering is not structural evidence.
4. Render the whole document once for overview, then render every known or affected page
   individually. Consume only exact `outputs[].readPath` values with `read_image.path` and keep a
   numbered ledger of pages actually read. The current semantic surface has no authoritative final
   page count, so do not claim exhaustive all-page coverage from a contact sheet or guessed count.
5. Compare requested changes and unrelated content with the baseline. Any final edit invalidates
   earlier inspection, validation, structural, and visual evidence; repeat the affected checks.
6. Report only checks that actually succeeded and disclose unsupported fidelity or unavailable
   visual coverage.

Read [references/workflows.md](references/workflows.md) for creation, input binding, observation,
render consumption, and quality checks. Read
[references/editing-existing.md](references/editing-existing.md) for every existing-document edit.
