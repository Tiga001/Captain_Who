# Spreadsheet workflows

## Choosing an execution path

Use the native path for workbook inspection, a small number of cell, range, formula, format, or
chart changes, and other bounded operations that `office_spreadsheet` represents directly. It
provides a frozen structured action, conflict checks, staging, and atomic publication for writes.

Use a reproducible script when the task is substantially clearer as code: importing or joining
data, populating large ranges, generating many formulas or styles, building repeated sheets and
charts, processing multiple workbooks, or maintaining a rerunnable workbook builder. Keep the
generator in a reviewable `.py` or `.mjs` file. A script is not a shortcut around file-edit or
command authorization; create or update it with `apply_patch` or `write_file`, then execute that
exact saved file with `run_command` and the managed Artifact Runtime.

The two paths compose. A script may produce the workbook, after which native range inspection,
rendering, and validation provide formula, structural, and visual verification. Prefer a new output
workbook for scripted transformations unless the user explicitly requested an in-place edit.

## Native tool contract

Call `office_spreadsheet` once per operation with exactly this root envelope: `{ "request": { "operation": "...", ... }, "reason": "..." }`. Put every operation and operation-specific field inside `request`. Keep `reason` as the only root field. Every call, including `status`, `help`, `get`, `query`, `validate`, `view`, and every mutation, must include it. Write `reason` as one non-empty, single-line plain-text sentence in the user's language, no longer than 240 characters, describing the user-visible purpose of this specific call. Do not include line breaks, control characters, or bidirectional text controls. Do not use it to assert success or authorization: it is untrusted display and audit metadata only and never grants permission, approval, or access.

The native request is typed and operation-specific. Put the workbook in `request.filePath`; use `request.target` for an exact workbook path, `request.parent` plus `request.element` for insertion, `request.properties` for typed values and formatting, and `request.outputPath` only for a rendering operation. Never send provider command tokens, document-format tokens, output flags, or a raw batch command. The host validates the typed request, generates deterministic provider argv, freezes it for approval, and regenerates it before execution.

Before the first workbook operation in a run, check the managed engine:

```json
{ "request": { "operation": "status" }, "reason": "Check whether spreadsheet tools are available" }
```

Disclose only the schema needed for the next change. Useful examples are:

```json
{
  "request": {
    "operation": "help",
    "verb": "add",
    "element": "cell"
  },
  "reason": "Check how to add the requested spreadsheet cells"
}
```

```json
{
  "request": {
    "operation": "help",
    "verb": "add",
    "element": "chart"
  },
  "reason": "Check how to add the requested spreadsheet chart"
}
```

Submit each supported native mutation separately and use the returned canonical path for follow-up operations when one is provided.

## Script authoring and observation contract

Select the `spreadsheets` runtime profile. The logical saved-script command selects its runtime
family; the model does not select a provider, runtime kind, dependency set, or version:

| Command  | Script | Profile libraries        | Use                                                                   |
| -------- | ------ | ------------------------ | --------------------------------------------------------------------- |
| `node`   | `.mjs` | `exceljs`                | Create or transform `.xlsx` workbooks with JavaScript.                |
| `python` | `.py`  | `openpyxl`, `xlsxwriter` | Edit with `openpyxl`; use `xlsxwriter` only to create a new workbook. |

Set the top-level `runtimeProfile` field to `spreadsheets`. Do not send a `runtime` object and do
not copy a provider, kind, package name, or package version into the tool call. The host maps
`node` or `python` to the matching profile entry, resolves the pinned dependencies, verifies the
managed runtime, and freezes the exact resolution and integrity identity before execution.

1. Put substantial program logic in a saved `.py` or `.mjs` file. Do not pass artifact-producing
   code through `python -c`, `node -e`, a heredoc, shell redirection, or another opaque inline form.
2. Make source data, workbook inputs, and output files explicit script parameters. Avoid hard-coded
   machine-specific paths, keep typed values distinct from display formats, and make a rerun
   deterministic where practical.
3. Execute the saved file through `run_command` with `runtimeProfile="spreadsheets"`. Dependency
   and runtime resolution is host-owned preflight, never implicit package installation.
4. Set `observe.kinds` to `["office"]`. List every `.xlsx` file that should be created or modified
   in `observe.expectedOutputs`. Paths are resolved relative to the command `cwd` and each expected
   output is observed directly. Do not add its parent merely because the output is outside the
   workspace. Use `observe.additionalRoots` only to scan for other Office changes in an otherwise
   uncovered directory; recursively observing an external directory requires `read=all`.
5. Treat observation as evidence, not authorization or transactionality. It never expands what the
   command may do and does not give an arbitrary script the native tool's staging or rollback
   guarantees.

For example, after creating `scripts/build_budget.py` with a file-editing tool, run the Python
entrypoint and observe its declared output:

```json
{
  "command": "python scripts/build_budget.py --output outputs/budget.xlsx",
  "cwd": ".",
  "runtimeProfile": "spreadsheets",
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/budget.xlsx"]
  },
  "reason": "Generate the requested workbook reproducibly"
}
```

The equivalent Node.js route keeps the same profile and changes only the reviewed script and its
logical entrypoint:

```json
{
  "command": "node scripts/build_budget.mjs --output outputs/budget.xlsx",
  "cwd": ".",
  "runtimeProfile": "spreadsheets",
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/budget.xlsx"]
  },
  "reason": "Generate the requested workbook reproducibly"
}
```

Keep `node` or `python` as the logical first command token. The host binds it to the profile's
managed executable; never discover or persist a private executable path. If profile preflight
fails, preserve that error. Do not remove `runtimeProfile`, use a system executable, install a
package, or guess another version. Use the native Office path only when it supports the requested
work, or rewrite and review a script for the other profile entrypoint before retrying with the same
profile. Never run a `.mjs` file as Python or a `.py` file as Node.js.

After every observed command, inspect `artifactObservation` even if the command failed, timed out,
or was cancelled:

- `status=complete` means the scan finished within its reported coverage policy, not that every
  filesystem entry was inspected. Always inspect excluded directories, warnings, and
  `changesTruncated`; `partial` or `failed` must be disclosed and cannot establish that no other
  Office file changed.
- Match every requested output in `artifactObservation.expectedOutputs`. Only `created`, `modified`,
  `replaced`, or `renamed` establish a file effect. `unchanged`, `missing`, `unobserved`, or
  `invalid` do not satisfy a requested edit.
- Inspect `changes` for unexpected workbook creation, replacement, deletion, or rename, and retain
  every warning. A non-zero exit can still leave file effects; a zero exit does not prove that the
  expected workbook exists or is valid.
- Do not blindly retry after any observed side effect. First inspect the resulting workbook and
  decide whether to continue from it, overwrite it deliberately, or report the partial outcome.

Observation is followed by workbook verification. Read representative ranges and exact formulas,
validate the package, and render every sheet or range whose layout matters. Do not infer formula
results that the selected library or engine did not calculate.

## Core recipes

Create a real workbook and rename its default sheet:

```json
{
  "request": {
    "operation": "create",
    "filePath": "budget.xlsx"
  },
  "reason": "Create the requested Excel workbook"
}
```

```json
{
  "request": {
    "operation": "set",
    "filePath": "budget.xlsx",
    "target": "/Sheet1",
    "properties": { "name": "预算" }
  },
  "reason": "Name the budget worksheet"
}
```

Write a typed value and a SUM formula. Formula text excludes the leading `=`:

```json
{
  "request": {
    "operation": "set",
    "filePath": "budget.xlsx",
    "target": "/预算/B2",
    "properties": {
      "value": 3000,
      "type": "number",
      "numberformat": "¥#,##0"
    }
  },
  "reason": "Write a formatted budget amount"
}
```

```json
{
  "request": {
    "operation": "set",
    "filePath": "budget.xlsx",
    "target": "/预算/E2",
    "properties": {
      "formula": "SUM(B2:D2)",
      "numberformat": "¥#,##0"
    }
  },
  "reason": "Add the Q1 total formula"
}
```

Format a header range and freeze the first row:

```json
{
  "request": {
    "operation": "set",
    "filePath": "budget.xlsx",
    "target": "/预算/A1:E1",
    "properties": {
      "fill": "1F4E78",
      "font.color": "FFFFFF",
      "bold": true
    }
  },
  "reason": "Format the workbook header"
}
```

```json
{
  "request": {
    "operation": "set",
    "filePath": "budget.xlsx",
    "target": "/预算",
    "properties": { "freeze": "A2" }
  },
  "reason": "Freeze the first worksheet row"
}
```

Add a column chart using category and Q1-total ranges that already exist:

```json
{
  "request": {
    "operation": "add",
    "filePath": "budget.xlsx",
    "parent": "/预算",
    "element": "chart",
    "properties": {
      "chartType": "column",
      "dataRange": "预算!E2:E4",
      "categories": "预算!A2:A4",
      "title": "Q1 合计",
      "anchor": "G2:N18"
    }
  },
  "reason": "Add the category Q1 total chart"
}
```

Read back the relevant range, including exact formula text and formats:

```json
{
  "request": {
    "operation": "get",
    "filePath": "budget.xlsx",
    "target": "/预算/A1:E4",
    "depth": 1
  },
  "reason": "Verify the budget values, formulas, and formats"
}
```

For the first save-as edit, keep `filePath` as the frozen source and supply a distinct
`destinationPath`. Apply later mutations to `budget-q1.xlsx` itself so earlier changes are
preserved:

```json
{
  "request": {
    "operation": "set",
    "filePath": "budget-template.xlsx",
    "destinationPath": "budget-q1.xlsx",
    "target": "/预算/E2",
    "properties": { "formula": "SUM(B2:D2)" }
  },
  "reason": "Create the Q1 workbook without overwriting the template"
}
```

Render the populated region to a workspace PNG:

```json
{
  "request": {
    "operation": "view",
    "filePath": "budget.xlsx",
    "mode": "screenshot",
    "range": "预算!A1:N18",
    "outputPath": "budget-preview.png"
  },
  "reason": "Render the workbook for visual inspection"
}
```

Validate the OOXML package and formula references:

```json
{
  "request": {
    "operation": "validate",
    "filePath": "budget.xlsx"
  },
  "reason": "Validate the finished Excel workbook"
}
```

## Create

1. Confirm sheet names, headers, data types, formulas, formats, filters, frozen panes, and chart requirements.
2. Choose native operations for bounded authoring or a saved generator for coordinated,
   repetitive authoring. Always create a real `.xlsx`; keep numeric and date values typed rather
   than storing them as formatted strings.
3. Write formulas into formula cells. Use absolute and relative references deliberately and keep ranges aligned with the data.
4. Apply number formats separately from values. Use reusable styles consistently across headers, totals, inputs, and outputs.
5. Add charts only after the source range is stable, and verify category and series references.

## Edit

1. Inspect relevant sheets, formulas, named ranges, tables, merged cells, and charts before mutation.
2. Preserve unrelated sheets, formulas, styles, workbook metadata, and embedded objects.
3. Prefer targeted native range or object changes for local edits. Use a saved script only when its
   coordinated transformation is materially clearer or more reproducible than many isolated calls.
4. With the native path, use `destinationPath` to save to a new workbook. With the script path,
   pass distinct input and output parameters. Edit in place only when it was explicitly requested.

## Verification

- Confirm required sheets and dimensions, representative cell values and types, exact formula text, number formats, frozen panes, and chart ranges.
- Check formula errors and validation warnings reported by the tool. Do not infer calculated values when the engine did not calculate them.
- Render the relevant sheet or range and inspect clipping, column widths, hidden content, chart labels, and totals.
- Treat a successful engine exit as necessary but insufficient when formulas or layout are material.
- Treat a successful script exit as necessary but insufficient; require matching complete artifact
  observation plus formula, structural, package, and visual checks appropriate to the task.
- Report only checks that actually ran and preserve warnings in the final result.
