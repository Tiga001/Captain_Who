# Spreadsheet workflows

## Contents

- [Route the task](#route-the-task)
- [Native semantic contract](#native-semantic-contract)
- [Create and reuse one Builder](#create-and-reuse-one-builder)
- [Use the Host syntax preflight](#use-the-host-syntax-preflight)
- [Edit an existing workbook safely](#edit-an-existing-workbook-safely)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Consume render outputs](#consume-render-outputs)
- [Generate, verify, render, iterate](#generate-verify-render-iterate)
- [Clean up managed scripts](#clean-up-managed-scripts)
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

For an existing workbook, inspect first and prefer a supported native operation for a bounded
change. The current Python Builder accepts `--source` and loads it with `openpyxl`, but it remains a
general Builder: there is no fixed existing-workbook Editor, Host-validated typed edit plan, or
whole-operation rollback contract. Use it only when the requested transformation and preservation
requirements are explicit and controllable. Default to save-as and fail closed rather than clear,
rebuild, flatten, or silently degrade an existing workbook.

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

File, image, CSV, and workbook inputs use one path string. For an attachment, call
`attachments_list` and copy its exact `readPath`; for a generated image, copy its exact
`image-artifact://...` path. If the backend returns
`office.capability_not_supported`, `capabilityNotSupported`, or
`recovery=useManagedScript`, preserve the error and use the Builder for creation. For an existing
workbook, switch only when a targeted `--source` transformation can preserve the inspected
features; otherwise fail closed. Do not repeat the same failed call with invented fields.

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
is not a substitute for the underlying value type. Each native mutation is an independent atomic
file transaction; multiple calls are not a single all-or-nothing batch. Re-inspect after removing
or moving a sheet before another operation whose target depends on sheet order.

## Create and reuse one Builder

The bundled `templates/builder.py` is a compact `openpyxl` starting point with typed values, a real
formula, formatting, conditional formatting, and a chart. Locate its exact revision-bound URI with
`skills_list_resources`.

Choose one dedicated workspace-relative script directory for the task. Reuse an existing plain
directory when appropriate. If it is absent, create it before materialization with a separate,
idempotent call:

```json
{
  "command": "mkdir -p workbook-work",
  "cwd": ".",
  "reason": "Create the workbook script directory before materializing the Builder"
}
```

Record whether this task created the directory. Then copy the Builder once into a new path inside
it:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/builder.py",
  "destination": "workbook-work/build_workbook.py",
  "reason": "Create a reviewable workbook builder from the activated Skill template"
}
```

Use the exact `sourceUri` returned by the resource list; the placeholder above is not a literal
URI. `skills_materialize_resource` is create-only. After materialization, patch
`workbook-work/build_workbook.py` and rerun that same file. Do not rematerialize over a modified Builder
or create a new script for every correction.

The bundled Builder route pins Python 3.12.13 with `openpyxl` 3.1.5 and `xlsxwriter` 3.2.9.

These versions describe the immutable profile; never send or install them. Run the materialized
template with a direct logical `python <script>.py --output <file.xlsx>` command. The Host verifies
the run-scoped materialization receipt, derives and freezes the `spreadsheets` profile from the
static Office output, and binds observation; omit `runtimeProfile` and `observe`. Never call a
private executable path, system Python, `pip`, inline code, a heredoc, or shell
redirection. Use `openpyxl` to edit existing workbooks; use `xlsxwriter` only for a new workbook.

## Use the Host syntax preflight

Every materialization or patch invalidates earlier syntax evidence. When a direct Builder command
is submitted, the Host automatically parses and compiles the exact saved Python Builder selected
by the trusted materialization receipt before execution. Do not ask the model to run a separate Python syntax command: never invoke system
Python, `python -m`, `python -c`, inline code, or a heredoc, and never combine a check and build with
`&&`, `|`, or `;`.

If automatic preflight reports a syntax error, the Builder did not execute. Patch that same file
and submit the direct Builder command again so the Host performs a fresh check. A successful
preflight proves only Python syntax; it does not prove runtime success, workbook validity,
calculation, or visual quality.

## Edit an existing workbook safely

Before an edit, inspect the source's sheet list, used ranges, formulas, tables, charts, names, and
other preservation-relevant features. For a bounded supported operation, use
`office_spreadsheet`. For a larger explicit transformation, bind exactly one source, load it with
the Builder's `--source` path, and write a distinct output:

```json
{
  "command": "python workbook-work/build_workbook.py --source source/template.xlsx --output outputs/template-edited.xlsx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source/template.xlsx",
      "path": "outputs/template.xlsx"
    }
  ],
  "reason": "Apply the requested workbook transformation to a save-as copy"
}
```

The bundled Builder's sample content is creation-oriented. Replace that sample with targeted
mutations; do not retain its sheet-clearing code for an edit. Replace or add task-specific semantic
assertions as needed, while keeping the template's generic reopen-before-publish check. Preserve
unrelated sheets, formulas, styles, names, charts, links, and media. If the workbook contains macros, signatures, external
connections, pivots or slicers, advanced drawings, or another feature whose round-trip fidelity
cannot be established, stop and report the limitation. Do not reconstruct the workbook and call
the result an edit. A Builder run observes its output but is not a Host-owned typed edit
transaction; do not claim whole-operation rollback or fixed-Editor guarantees.

## Bind inputs declaratively

Every input needed by a Builder must be explicit in `run_command.inputs`:

```json
{
  "command": "python workbook-work/build_workbook.py --source source/template.xlsx --output outputs/budget.xlsx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source/template.xlsx",
      "path": "outputs/template.xlsx"
    }
  ],
  "reason": "Build the budget workbook and track its output"
}
```

Supported paths are workspace-relative paths, authorized absolute/system paths, exact attachment
`readPath` values, exact generated `image-artifact://...` paths, and exact revision-bound
`skill://...` URIs. The Host resolves the internal source type.

`mountPath` is an optional private input-root-relative filename and defaults to the source filename. The Host freezes and revalidates the input,
then exposes the run-scoped root in `MYCOPILOT_INPUT_ROOT`. The Builder resolves
`MYCOPILOT_INPUT_ROOT / mountPath`. Never let Python open `@attachments`, `skill://`, an
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

After the last workbook edit, inspect again and record the final sheet list, each sheet's complete
populated used range, and every reported floating chart or image anchor. When precise floating
object bounds are available, define the sheet's visual extent as their union with the populated
used range. Render that complete extent to an explicit review image for every final sheet:

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
- Maintain a ledger with one entry per final sheet: sheet name, inspected complete used range,
  reported chart/image anchors, resolved visual extent, render request, exact returned `readPath`,
  and visual verdict. An empty sheet may be recorded as empty only when final inspect establishes
  that fact.
- `pageSelection` records the requested render selection; it is not independent proof of sheet,
  range, floating-object, or layout coverage. Coverage requires matching the render request to the
  final inspected sheet and resolved visual extent, then successfully reading the returned image.
- A populated used range alone is insufficient when a chart or picture floats outside those cells.
  If inspect cannot provide the bounds needed to include every reported floating object, still
  review available evidence but mark that sheet's complete visual coverage unavailable.
- If the output is absent, not readable, or `read_image` reports an unsupported model capability,
  state that visual verification was unavailable.
- If a complete used-range preview exceeds the visual-input limit, mark that sheet's visual
  verification unavailable. Do not fragment the range and claim a whole-sheet verdict or claim
  inspection of an unread image.
- Any workbook edit invalidates the entire ledger. Re-inspect the final workbook and rebuild every
  sheet entry instead of keeping verdicts from an earlier revision.

## Generate, verify, render, iterate

Use this fixed loop for a final workbook:

1. Generate or edit the `.xlsx`.
2. Confirm the expected file effect.
3. Inspect the final sheet list, each complete populated used range, and reported chart/image
   anchors, plus representative typed values, exact formulas, number formats, frozen panes,
   conditional formatting, tables, and chart source ranges.
4. Run native `validate`. This checks package structure, not formula evaluation.
5. Resolve each final worksheet's visual extent from the used range plus all reported floating
   object bounds, render that extent, and build the per-sheet visual ledger. If any object bound is
   unavailable, disclose incomplete coverage; do not infer it from `pageSelection` or a filename.
6. Read every exact returned render output with `read_image`, then record one verdict per final
   sheet for labels, column widths, wrapped text, hidden or clipped content, totals, chart
   placement, legends, and axis labels.
7. If a defect exists, patch the same Builder or issue one corrected semantic operation,
   regenerate, then discard the ledger and repeat inspect, validation, rendering, and review.

Do not claim visual quality from package validation alone. If rendering returns an
`office.render_backend_*` error, report that visual verification was unavailable; do not launch a
user browser or fragment one range into repeated retry calls.

Keep calculation claims separate. `openpyxl` reads and writes formula expressions but does not
evaluate them. Exact formula inspection proves only the stored expression; `validate` proves only
package validity; source cached values can be missing or stale. Claim recalculated values only
when a calculation engine returned explicit authoritative evidence. If no such evidence exists,
say that formulas were preserved or written and require recalculation in Excel—never say that
`openpyxl` calculated them.

## Clean up managed scripts

After the final inspect, validation, visual ledger, and any retries are complete, delete the exact
task-owned Builder and task-created temporary files. Preserve unrelated and pre-existing files.
If this task created the script directory and it is now empty, remove it with a separate `rmdir`
command; otherwise leave it in place. Never use `rm -rf` or another recursive deletion for this
cleanup. Keep the Builder only when the user explicitly asks for it.

## Spreadsheet quality checks

- Store formulas as formulas, never as displayed numeric results.
- Do not claim that `openpyxl`, formula inspection, cached values, or package validation performed
  formula calculation.
- Apply number formats separately from typed values.
- Keep formula references aligned with the final data range.
- Preserve unrelated sheets, formulas, styles, names, charts, and embedded media during edits.
- Fail closed when an existing feature cannot be round-tripped and verified; never rebuild an
  existing workbook to imitate an edit.
- Verify chart categories and series against exact source ranges.
- Require one visual-ledger verdict for every final sheet whose full visual extent is resolvable;
  otherwise record the sheet with an explicit incomplete-coverage verdict.
- Report formula or validation warnings and only checks that actually ran.
