---
name: spreadsheets
description: Create, edit, inspect, calculate, and render Microsoft Excel-compatible .xlsx workbooks. Use for formulas, typed data, formatting, tables, charts, images, multiple sheets, repeated or data-driven generation, and any other spreadsheet task.
---

# Spreadsheets

Route by intent:

- **Read and verify:** use the flat semantic `office_spreadsheet` tool only for `inspect` and `render`.
- **Create:** use one saved Managed Builder from `templates/builder.py` with normal Python and the pinned managed spreadsheet runtime.
- **Edit an existing `.xlsx`:** inspect first, then use one saved Workbook Editor from
  `templates/editor.py`. It executes normal Python against the frozen source snapshot and publishes
  a distinct Host-gated save-as output.

For a workbook attachment, use `attachments_list` for this conversation or
`attachments_list_project` for authorized project/task-tree attachments. Pass its exact returned
`readPath` to the available `office_spreadsheet` reader; when using a script, bind that same path
through `run_command.inputs`. These virtual paths are not workspace files. Use only tools
actually provided in the current model request; do not invent a missing reader or Skill ref.

Do not call or invent native Office create or mutation operations. The model-facing Office tool is intentionally read/verify-only; all workbook writes go through the fixed Builder or Editor.

## Managed scripts

Before materializing, choose one dedicated workspace-relative script directory. Reuse an existing plain directory when appropriate; otherwise create it first with one separate idempotent `mkdir -p <directory>` command. Record whether this task created it.

Use `skills_list_resources` to locate `templates/builder.py` for creation or `templates/editor.py` for an existing workbook. Materialize the chosen exact revision once into a new path in that prepared directory. Patch and rerun that same file; do not accumulate replacement scripts.

Both files are genuine Python. The managed Python 3.12.13 spreadsheet runtime provides `openpyxl` 3.1.5, `xlsxwriter` 3.2.9, `numpy` 2.5.2, and `pandas` 3.0.5. `openpyxl` remains the workbook authoring and Host validation engine; the additional libraries do not change the Builder/Editor workflow. The Editor's `edit_workbook` region may contain functions, loops, conditions, comprehensions, and imports from the pinned runtime. Never convert it into an AST, JSON, or artificial operation DSL. Keep the fixed CLI, input resolver, publication code, and edit markers unchanged.

Rely on the Host's automatic syntax preflight after every materialization or patch. Do not issue a model-authored syntax-check command; never request system Python, `python -m`, `python -c`, inline code, heredocs, package installation, or a command combined with `&&`, `|`, or `;`.

Run one direct logical command:

- Create: `python <builder>.py --output <file.xlsx>`
- Edit: `python <editor>.py --source <logical-input.xlsx> --output <distinct-file.xlsx>`

Every Builder or Editor command must declare exactly one static `--output`.

Omit `runtimeProfile` and `observe`. Bind every source workbook, image, CSV, attachment, or earlier
artifact through `run_command.inputs` and use its logical `mountPath` below
`MYCOPILOT_INPUT_ROOT`. The Host freezes the script, runtime, inputs, and target; redirects the
declared Office output through private staging; reopens it with pinned `openpyxl` using
`data_only=False`; and publishes atomically. Do not duplicate that gate in the script or call
native `validate`. The fixed script saves once to the Host-provided output path; do not add another
temporary file, candidate reopen, or `os.replace` layer. If a command returns `running`, wait on
that same command session before using the output. Inspect `artifactObservation` after every
terminal result and never blindly repeat a call that may have changed files.

The Host verifies this Run's materialization receipt and freezes the `spreadsheets` runtime,
version, and integrity. It grants no additional command or file permission and never falls back to
executables on PATH. Never supply package versions, guess a profile, or use `runtimeProfile` to
replace the fixed Builder/Editor workflow with a self-authored script.

`observe` is optional best-effort Office file observation, not permission or command-success
evidence. For a separately needed explicit observation, use `kinds: ["office"]` and exact
`expectedOutputs`, resolved relative to `cwd`; it does not enumerate sibling files. Add
`additionalRoots` only for separately authorized files/directories; recursive scans outside the
workspace require `read=all`. Verified Builder/Editor outputs are observed automatically, so keep
omitting `observe` for the normal workflow.

`openpyxl` stores formulas but does not calculate them. The Host reopen gate, formula inspection,
and cached values do not prove recalculation. Claim calculated results only when a separate
authoritative calculation engine returned explicit evidence.

## Completion gate

1. Inspect the source before editing and stop when preservation of an unsupported workbook feature cannot be established.
2. Create or edit with one task-owned managed script; keep values, dates, booleans, and formulas typed.
3. Require terminal command success and an `artifactObservation` confirming the expected `.xlsx` effect.
4. Terminal success proves the Host reopened the private candidate with pinned `openpyxl` before
   publication. Inspect the final sheet list, populated ranges, formula text, formats, tables,
   charts, images, and defined names; do not call native `validate`.
5. For every final sheet, render the visual extent formed by its populated range plus every reported floating chart/image bound. Read each exact returned `outputs[].readPath` with the actually available `read_image.path` and record a verdict. If bounds, rendering, or image reading are unavailable, disclose incomplete coverage. Any later edit invalidates the ledger.
6. Keep syntax, execution, package, calculation, and visual evidence separate. Report only checks that actually succeeded.
7. Delete the exact task-owned Builder or Editor and temporary files. Use `rmdir` only if this task created the now-empty directory; preserve pre-existing files and never use recursive deletion.

Read [references/workflows.md](references/workflows.md) for creation, inputs, observation, and QA. Before editing an existing workbook, also read [references/editing-existing.md](references/editing-existing.md).
