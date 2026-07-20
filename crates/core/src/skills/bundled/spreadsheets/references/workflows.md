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

Call `office_spreadsheet` with one JSON object per operation. `arguments` is an array of literal OfficeCLI tokens, not a shell command: each flag and each flag value is a separate array entry. Never add shell quoting, redirects, pipes, an executable name, or a raw batch command.

Before the first workbook operation in a run, check the managed engine:

```json
{ "operation": "status" }
```

Disclose only the schema needed for the next change. Useful examples are:

```json
{ "operation": "help", "arguments": ["xlsx", "add", "cell", "--json"] }
```

```json
{ "operation": "help", "arguments": ["xlsx", "add", "chart", "--json"] }
```

Submit each supported native mutation separately and use the returned canonical path for follow-up operations when one is provided.

## Script authoring and observation contract

The managed Artifact Runtime exposes these spreadsheet libraries:

| Runtime | Script | Package requirement  | Use                                                                                             |
| ------- | ------ | -------------------- | ----------------------------------------------------------------------------------------------- |
| Node.js | `.mjs` | `exceljs` `4.4.0`    | Create or transform `.xlsx` workbooks with JavaScript.                                          |
| Python  | `.py`  | `openpyxl` `3.1.5`   | Create or edit `.xlsx` workbooks while preserving workbook structures supported by the library. |
| Python  | `.py`  | `xlsxwriter` `3.2.9` | Create new `.xlsx` workbooks; do not select it for editing an existing workbook.                |

Declare only the packages the selected script actually imports. Package names and versions belong
in `runtime.requiredPackages`; never install them from the script.

1. Put substantial program logic in a saved `.py` or `.mjs` file. Do not pass artifact-producing
   code through `python -c`, `node -e`, a heredoc, shell redirection, or another opaque inline form.
2. Make source data, workbook inputs, and output files explicit script arguments. Avoid hard-coded
   machine-specific paths, keep typed values distinct from display formats, and make a rerun
   deterministic where practical.
3. Execute the saved file through `run_command` with the managed Artifact Runtime. Declare every
   runtime package the script requires; dependency resolution is preflight, never implicit package
   installation.
4. Set `observe.kinds` to `["office"]`. List every `.xlsx` file that should be created or modified
   in `observe.expectedOutputs`. Paths are resolved relative to the command `cwd` and each expected
   output is observed directly. Do not add its parent merely because the output is outside the
   workspace. Use `observe.additionalRoots` only to scan for other Office changes in an otherwise
   uncovered directory; recursively observing an external directory requires `read=all`.
5. Treat observation as evidence, not authorization or transactionality. It never expands what the
   command may do and does not give an arbitrary script the native tool's staging or rollback
   guarantees.

For example, after creating `scripts/build_budget.py` with a file-editing tool, execute that exact
file with the managed Python runtime and observe its declared output:

```json
{
  "command": "python scripts/build_budget.py --output outputs/budget.xlsx",
  "cwd": ".",
  "runtime": {
    "provider": "managedArtifact",
    "kind": "python",
    "requiredPackages": [{ "name": "openpyxl", "version": "3.1.5" }]
  },
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/budget.xlsx"]
  },
  "reason": "Generate the requested workbook reproducibly"
}
```

Keep `python` as the logical first command token. The host binds it to the selected managed runtime;
never discover or persist the runtime's private executable path in the script or command.

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
  "operation": "create",
  "path": "budget.xlsx",
  "arguments": ["--json"],
  "reason": "Create the requested Excel workbook"
}
```

```json
{
  "operation": "set",
  "path": "budget.xlsx",
  "arguments": ["/Sheet1", "--prop", "name=预算", "--json"],
  "reason": "Name the budget worksheet"
}
```

Write a typed value and a SUM formula. Formula text excludes the leading `=`:

```json
{
  "operation": "set",
  "path": "budget.xlsx",
  "arguments": [
    "/预算/B2",
    "--prop",
    "value=3000",
    "--prop",
    "type=number",
    "--prop",
    "numberformat=¥#,##0",
    "--json"
  ],
  "reason": "Write a formatted budget amount"
}
```

```json
{
  "operation": "set",
  "path": "budget.xlsx",
  "arguments": [
    "/预算/E2",
    "--prop",
    "formula=SUM(B2:D2)",
    "--prop",
    "numberformat=¥#,##0",
    "--json"
  ],
  "reason": "Add the Q1 total formula"
}
```

Format a header range and freeze the first row:

```json
{
  "operation": "set",
  "path": "budget.xlsx",
  "arguments": [
    "/预算/A1:E1",
    "--prop",
    "fill=1F4E78",
    "--prop",
    "font.color=FFFFFF",
    "--prop",
    "bold=true",
    "--json"
  ],
  "reason": "Format the workbook header"
}
```

```json
{
  "operation": "set",
  "path": "budget.xlsx",
  "arguments": ["/预算", "--prop", "freeze=A2", "--json"],
  "reason": "Freeze the first worksheet row"
}
```

Add a column chart using category and Q1-total ranges that already exist:

```json
{
  "operation": "add",
  "path": "budget.xlsx",
  "arguments": [
    "/预算",
    "--type",
    "chart",
    "--prop",
    "chartType=column",
    "--prop",
    "dataRange=预算!E2:E4",
    "--prop",
    "categories=预算!A2:A4",
    "--prop",
    "title=Q1 合计",
    "--prop",
    "anchor=G2:N18",
    "--json"
  ],
  "reason": "Add the category Q1 total chart"
}
```

Read back the relevant range, including exact formula text and formats:

```json
{
  "operation": "get",
  "path": "budget.xlsx",
  "arguments": ["/预算/A1:E4", "--depth", "1", "--json"]
}
```

For the first save-as edit, keep `path` as the frozen source and supply a distinct
`destinationPath`. Apply later mutations to `budget-q1.xlsx` itself so earlier changes are
preserved:

```json
{
  "operation": "set",
  "path": "budget-template.xlsx",
  "destinationPath": "budget-q1.xlsx",
  "arguments": ["/预算/E2", "--prop", "formula=SUM(B2:D2)", "--json"],
  "reason": "Create the Q1 workbook without overwriting the template"
}
```

Render the populated region to a workspace PNG:

```json
{
  "operation": "view",
  "path": "budget.xlsx",
  "arguments": ["screenshot", "--range", "预算!A1:N18", "--json"],
  "outputPath": "budget-preview.png",
  "reason": "Render the workbook for visual inspection"
}
```

Validate the OOXML package and formula references:

```json
{ "operation": "validate", "path": "budget.xlsx", "arguments": ["--json"] }
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
   pass distinct input and output arguments. Edit in place only when it was explicitly requested.

## Verification

- Confirm required sheets and dimensions, representative cell values and types, exact formula text, number formats, frozen panes, and chart ranges.
- Check formula errors and validation warnings reported by the tool. Do not infer calculated values when the engine did not calculate them.
- Render the relevant sheet or range and inspect clipping, column widths, hidden content, chart labels, and totals.
- Treat a successful engine exit as necessary but insufficient when formulas or layout are material.
- Treat a successful script exit as necessary but insufficient; require matching complete artifact
  observation plus formula, structural, package, and visual checks appropriate to the task.
- Report only checks that actually ran and preserve warnings in the final result.
