# Spreadsheet workflows

## Contents

- [Route the task](#route-the-task)
- [Use the read-only Office contract](#use-the-read-only-office-contract)
- [Create and reuse one Builder](#create-and-reuse-one-builder)
- [Use the fixed Editor](#use-the-fixed-editor)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Use the Host syntax and transaction gates](#use-the-host-syntax-and-transaction-gates)
- [Inspect every file effect](#inspect-every-file-effect)
- [Verify calculation and visual quality](#verify-calculation-and-visual-quality)
- [Clean up managed scripts](#clean-up-managed-scripts)

## Route the task

Use `office_spreadsheet` only for the read/verification operations `inspect` and `render`. Do not
call `status`, `validate`, `create`, `addSheet`, `writeCell`, `setFormula`,
`formatRange`, `freezePanes`, `addConditionalFormat`, `addTable`, `addChart`,
`insertImage`, `removeSheet`, `moveSheet`, or any invented low-level operation.

Use the fixed Python Managed Builder or Editor for every write:

- Materialize `templates/builder.py` to create a new `.xlsx`.
- Inspect first, then materialize `templates/editor.py` to edit an existing `.xlsx`.

The Builder and Editor are separate on purpose. Never load an existing workbook in the
creation Builder, clear its content, or reconstruct it to imitate an edit.

## Use the read-only Office contract

Call `office_spreadsheet` with a flat object and one concise `reason`:

```json
{
  "operation": "inspect",
  "filePath": "budget.xlsx",
  "sheetName": "季度预算",
  "range": "A1:J30",
  "reason": "Inspect the final budget values, formulas, and visual extent"
}
```

Keep `operation`, paths, selectors, and `reason` at the root. Never add a `request`
wrapper, provider arguments, executable paths, workbook DOM paths, or arbitrary property
maps. A `reason` is user-facing audit text, not permission.

Use inspect before editing to record every preservation-sensitive feature: final sheet
identity and order, used ranges, typed values, formulas, formats, tables, charts, images,
defined names, validations, links, hidden state, and frozen panes.

## Create and reuse one Builder

Choose one dedicated task script directory. Reuse an existing plain directory when
appropriate. If it does not exist, create it before materialization:

```json
{
  "command": "mkdir -p workbook-work",
  "cwd": ".",
  "reason": "Create the workbook script directory before materializing the Builder"
}
```

Record whether this task created the directory. Locate the exact revision-bound
`templates/builder.py` URI with `skills_list_resources`, then materialize it once:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/builder.py",
  "destination": "workbook-work/build_workbook.py",
  "reason": "Create a reviewable Python workbook Builder"
}
```

Use the exact returned URI; the example URI is not literal. Patch the same Builder and
run one direct command:

```json
{
  "command": "python workbook-work/build_workbook.py --output budget.xlsx",
  "cwd": ".",
  "reason": "Create the requested workbook with the managed Python Builder"
}
```

Omit `runtimeProfile` and `observe`. The exact materialization receipt binds the pinned Python
3.12.13 runtime with `openpyxl` 3.1.5, `xlsxwriter` 3.2.9, `numpy` 2.5.2, and `pandas` 3.0.5.
`openpyxl` remains the workbook authoring and Host validation engine; the available library set
does not change the managed Builder/Editor route.

## Use the fixed Editor

For an existing workbook, read [editing-existing.md](editing-existing.md). Prepare a task
directory, materialize `templates/editor.py` once, and patch only `edit_workbook`:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/editor.py",
  "destination": "workbook-work/edit_workbook.py",
  "reason": "Create the fixed Python Editor for the existing workbook"
}
```

The edit region is normal Python. Use functions, loops, conditions, comprehensions, and
pinned libraries when they make the transformation safer or clearer. Do not encode the
task as JSON or as a fake operation list. Keep the wrapper and markers unchanged.

Run exactly one source and one distinct output:

```json
{
  "command": "python workbook-work/edit_workbook.py --source source.xlsx --output budget-edited.xlsx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source.xlsx",
      "path": "budget.xlsx"
    }
  ],
  "reason": "Edit the existing workbook and publish a Host-gated save-as copy"
}
```

Editor v1 is save-as only. Preserve the source and unrelated workbook content. Stop when
macros/signatures, Power Query, external connections, pivots/slicers, advanced drawings,
embedded objects, or another unsupported feature cannot be preserved and verified.

## Bind inputs declaratively

Declare every source workbook, CSV, image, attachment, or earlier artifact in
`run_command.inputs`:

```json
{
  "mountPath": "assets/logo.png",
  "path": "image-artifact://example"
}
```

The Host accepts authorized workspace or absolute paths, attachment `readPath` values,
generated artifact URIs, and revision-bound Skill URIs. `mountPath` is the logical
input-root-relative name used by the script. Never open an attachment URI, Skill URI, or
private storage path directly; resolve declared inputs below `MYCOPILOT_INPUT_ROOT`.

## Use the Host syntax and transaction gates

Every materialization or patch invalidates prior syntax evidence. Submit the direct
Builder or Editor command and let the Host preflight the exact frozen Python file. Never
run system Python, `python -m`, `python -c`, inline code, heredocs, package installation,
or a syntax/build command joined with `&&`, `|`, or `;`.

For a recognized Office script, the Host freezes the script bytes, runtime receipt,
inputs, and target precondition; redirects the declared output to private staging; runs
the genuine Python program under the ordinary `run_command` approval and permission
boundary; reopens the candidate with pinned `openpyxl.load_workbook(data_only=False)`;
rechecks source, target, and cancellation state; and publishes atomically. The Host owns this
prepublish reopen. Do not duplicate it in the script or call native `validate`. A failed preflight,
runtime error, timeout, cancellation, reopen, or target conflict must not publish the declared
final output.

This is a transactional Office-output guarantee, not a claim that unrestricted Python is
a separate cross-platform OS sandbox.

## Inspect every file effect

Every Builder or Editor command must contain exactly one static `.xlsx` `--output`.
The Host supplies Office artifact observation automatically. Inspect
`artifactObservation` after success, failure, timeout, or cancellation:

- Require the expected output to be `created`, `modified`, `replaced`, or `renamed` and
  valid before delivery.
- Treat `missing`, `unchanged`, `unobserved`, invalid, or partial observation as
  insufficient.
- Report unexpected file effects.
- If the command returns `running`, wait on that same command session; do not start QA or
  link the target before terminal success.
- Never blindly rerun after a command that may have produced a file effect.

Observation records evidence. It does not replace the Host prepublish reopen or visual review.

## Verify calculation and visual quality

After the final write:

1. Inspect the final sheet list and order, each complete populated range, typed values,
   exact formula text, formats, names, tables, charts, images, validations, and links.
2. Confirm terminal success from the Host's pinned openpyxl reopen gate. Do not call native
   `validate`; the model's Office workflow is inspect/render only.
3. Build a visual ledger for every final sheet. Resolve its visual extent as the union of
   the populated range and every reported floating chart/image bound, then render it.
4. Read the exact returned render `outputs[].readPath` with `read_image.path`. Never infer
   the path from the request, stdout, cwd, or filename.
5. Record one verdict per final sheet. If floating-object bounds, rendering, or visual
   input are unavailable, disclose incomplete coverage rather than claiming success.
6. Any later edit invalidates the ledger; require a new successful Host publication, then
   re-inspect, render, and review again.

`openpyxl` stores formula expressions but does not calculate them. Cached values may be
missing or stale. Claim recalculation only when a separate authoritative calculation
engine returned explicit evidence; otherwise say Excel must recalculate the formulas when
opened.

## Clean up managed scripts

After final QA, delete the exact task-owned Builder or Editor and task-created temporary
files. Preserve unrelated and pre-existing files. If this task created the script
directory and it is empty, remove it with a separate `rmdir`; otherwise leave it in
place. Never use `rm -rf` or another recursive deletion. Keep the script only when the
user explicitly requests it.
