# Spreadsheet workflows

## Contents

- [Route the task](#route-the-task)
- [Native semantic contract](#native-semantic-contract)
- [Create and reuse one Builder](#create-and-reuse-one-builder)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Consume render outputs](#consume-render-outputs)
- [Generate, verify, render, iterate](#generate-verify-render-iterate)
- [Spreadsheet quality checks](#spreadsheet-quality-checks)

## Route the task

Use `office_spreadsheet` first when the request is a bounded combination of its semantic
operations:

`create`, `inspect`, `validate`, `render`, `addSheet`, `writeCell`, `setFormula`, `formatRange`,
`freezePanes`, `addConditionalFormat`, `addTable`, `addChart`, `insertImage`, `removeSheet`, and
`moveSheet`.

Use the Managed Builder for large ranges, many formulas or styles, coordinated multi-sheet models,
imports, repeated charts, template population, batch generation, or a semantic operation that the
backend reports as unsupported. Do not emulate an unsupported operation with low-level OfficeCLI
fields. A hybrid flow—Builder write, native formula inspection/validation/render—is usually best
for complex workbooks.

## Native semantic contract

Call `office_spreadsheet` with a flat object:

```json
{
  "operation": "create",
  "filePath": "outputs/budget.xlsx",
  "reason": "Create the requested Excel budget workbook"
}
```

Keep `operation`, its semantic fields, and `reason` at the root. Never add a `request` wrapper,
provider arguments, an executable, workbook DOM paths, or shell flags. `reason` is required,
user-visible audit text only; it never grants permission.

File, image, CSV, and workbook inputs use the shared `AgentFileInputRef` object rather than guessed
paths. For an attachment, call `attachments_list` and copy its exact `readPath`. For an earlier
generated image, use a `generated_artifact` reference. If the backend returns
`office.capability_not_supported`, `capabilityNotSupported`, or
`recovery=useManagedScript`, preserve the error and switch to the Builder path. Do not repeat the
same failed call with invented fields.

Write a real formula using a semantic cell reference:

```json
{
  "operation": "setFormula",
  "filePath": "outputs/budget.xlsx",
  "sheetName": "季度预算",
  "cell": "E2",
  "formula": "SUM(B2:D2)",
  "numberFormat": "¥#,##0.00",
  "reason": "Add the Q1 total formula to the first budget row"
}
```

Inspect an existing workbook before mutation. Prefer save-as for transformations unless the user
explicitly requests in-place editing. Keep numbers, dates, booleans, and formulas typed; formatting
is not a substitute for the underlying value type.

## Create and reuse one Builder

The bundled `templates/builder.py` is a compact `openpyxl` starting point with typed values, a real
formula, formatting, conditional formatting, and a chart. Locate its exact revision-bound URI with
`skills_list_resources`, then copy it once into a new workspace path:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/builder.py",
  "destination": "scripts/build_workbook.py",
  "reason": "Create a reviewable workbook builder from the activated Skill template"
}
```

Use the exact `sourceUri` returned by the resource list; the placeholder above is not a literal
URI. `skills_materialize_resource` is create-only. After materialization, patch
`scripts/build_workbook.py` and rerun that same file. Do not rematerialize over a modified Builder
or create a new script for every correction.

The host-owned `spreadsheets` profile pins:

- Python 3.12.13 with `openpyxl` 3.1.5 and `xlsxwriter` 3.2.9.
- Node.js 22.23.1 with `exceljs` 4.4.0.

These versions describe the immutable profile; never send or install them. Run the materialized
template with a direct logical `python <script>.py --output <file.xlsx>` command. The Host verifies
the run-scoped materialization receipt, derives and freezes the `spreadsheets` profile from the
static Office output, and binds observation; omit `runtimeProfile` and `observe`. Never call a
private executable path, system Python/Node.js, `pip`, `npm`, inline code, a heredoc, or shell
redirection. Use `openpyxl` to edit existing workbooks; use `xlsxwriter` only for a new workbook.

## Bind inputs declaratively

Every input needed by a Builder must be explicit in `run_command.inputs`:

```json
{
  "command": "python scripts/build_workbook.py --source source/template.xlsx --output outputs/budget.xlsx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source/template.xlsx",
      "source": {
        "type": "workspace",
        "path": "outputs/template.xlsx"
      }
    }
  ],
  "reason": "Build the budget workbook and track its output"
}
```

Supported source types are:

- `attachment`: exact registered `readPath`.
- `workspace`: workspace file `path`.
- `external`: authorized external file `path`.
- `generated_artifact`: exact image Artifact `uri` plus the exact absolute `savedPath` as `path`.
- `skill_resource`: exact revision-bound `uri`.

`mountPath` is a private input-root-relative filename. The Host freezes and revalidates the input,
then exposes the run-scoped root in `MYCOPILOT_INPUT_ROOT`. The Builder resolves
`MYCOPILOT_INPUT_ROOT / mountPath`. Never let Python or Node.js open `@attachments`, `skill://`, an
attachment library path, or another private storage path directly.

## Observe every file effect

Every Builder command must declare its expected `.xlsx` with exactly one static `--output`
argument. The Host automatically binds `observe.kinds=["office"]` and copies that path into
`observe.expectedOutputs`. Paths are relative to `cwd` unless workspace-external output is
authorized with `write=all`. Use an explicit `observe.additionalRoots` only to discover other
Office changes that are not already named; do not scan a whole external directory merely because
one expected output is external.

Inspect `artifactObservation` after success, non-zero exit, timeout, and cancellation:

- Match every requested file in `artifactObservation.expectedOutputs`.
- Accept `created`, `modified`, `replaced`, or `renamed` as evidence of a file effect.
- Treat `missing`, `unchanged`, `unobserved`, `invalid`, partial coverage, or failed observation as
  insufficient.
- Report unexpected creations, replacements, deletions, or renames.
- Never blindly rerun after a command that may have produced side effects.

Observation records effects; it does not grant access, make arbitrary scripts transactional, or
prove that the workbook is valid.

## Consume render outputs

Render a populated sheet or range to an explicit review image:

```json
{
  "operation": "render",
  "filePath": "outputs/budget.xlsx",
  "outputPath": "outputs/budget-preview.png",
  "sheetName": "季度预算",
  "range": "A1:J30",
  "reason": "Render the completed budget sheet for visual review"
}
```

After a successful render, take the exact path from `outputs[].readPath` and pass it as
`read_image.path`:

```json
{
  "path": "outputs/budget-preview.png"
}
```

Treat the returned output as authoritative:

- Select the output whose `role` is `render` and whose `kind` is `image`.
- Use only its `readPath`; never reconstruct a path from the render request, `source`, `argv`,
  `cwd`, `stdout`, or a file search.
- Never rerender merely to discover where the first render was published.
- `pageSelection` records the requested render selection; it is not an independent proof of sheet,
  range, or layout coverage. Establish the intended sheet and used range with `inspect`, compare
  them with the render request, and then inspect the returned image.
- If the output is absent, not readable, or `read_image` reports an unsupported model capability,
  state that visual verification was unavailable.
- If a preview exceeds the visual-input limit, render bounded complete ranges or sheet groups
  rather than repeating the same oversized request. Do not claim inspection of unread images.

## Generate, verify, render, iterate

Use this fixed loop for a final workbook:

1. Generate or edit the `.xlsx`.
2. Confirm the expected file effect.
3. Inspect required sheet names, dimensions, representative typed values, exact formulas, number
   formats, frozen panes, conditional formatting, tables, and chart source ranges.
4. Run native `validate`; do not invent cached formula values the selected engine did not compute.
5. Render every final worksheet once. Use one combined request when the semantic renderer accepts
   multiple sheets; otherwise render one complete populated range per sheet without fragmenting it.
6. Read each exact returned render output with `read_image`, then visually inspect labels, column
   widths, wrapped text, hidden or clipped content, totals, chart placement, legends, and axis
   labels.
7. If a defect exists, patch the same Builder or issue one corrected semantic operation,
   regenerate, and repeat validation and rendering.

Do not claim visual quality from package validation alone. If rendering returns an
`office.render_backend_*` error, report that visual verification was unavailable; do not launch a
user browser or fragment one range into repeated retry calls.

## Spreadsheet quality checks

- Store formulas as formulas, never as displayed numeric results.
- Apply number formats separately from typed values.
- Keep formula references aligned with the final data range.
- Preserve unrelated sheets, formulas, styles, names, charts, and embedded media during edits.
- Verify chart categories and series against exact source ranges.
- Report formula or validation warnings and only checks that actually ran.
