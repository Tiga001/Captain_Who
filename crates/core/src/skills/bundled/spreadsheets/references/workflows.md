# Spreadsheet workflows

## Native tool contract

Call `office_spreadsheet` with one JSON object per operation. `arguments` is an array of literal OfficeCLI tokens, not a shell command: each flag and each flag value is a separate array entry. Never add shell quoting, redirects, pipes, an executable name, or a raw batch command.

Before the first workbook operation in a run, check the managed engine:

```json
{"operation":"status"}
```

Disclose only the schema needed for the next change. Useful examples are:

```json
{"operation":"help","arguments":["xlsx","add","cell","--json"]}
```

```json
{"operation":"help","arguments":["xlsx","add","chart","--json"]}
```

Submit each supported native mutation separately and use the returned canonical path for follow-up operations when one is provided.

## Core recipes

Create a real workbook and rename its default sheet:

```json
{"operation":"create","path":"budget.xlsx","arguments":["--json"],"reason":"Create the requested Excel workbook"}
```

```json
{"operation":"set","path":"budget.xlsx","arguments":["/Sheet1","--prop","name=预算","--json"],"reason":"Name the budget worksheet"}
```

Write a typed value and a SUM formula. Formula text excludes the leading `=`:

```json
{"operation":"set","path":"budget.xlsx","arguments":["/预算/B2","--prop","value=3000","--prop","type=number","--prop","numberformat=¥#,##0","--json"],"reason":"Write a formatted budget amount"}
```

```json
{"operation":"set","path":"budget.xlsx","arguments":["/预算/E2","--prop","formula=SUM(B2:D2)","--prop","numberformat=¥#,##0","--json"],"reason":"Add the Q1 total formula"}
```

Format a header range and freeze the first row:

```json
{"operation":"set","path":"budget.xlsx","arguments":["/预算/A1:E1","--prop","fill=1F4E78","--prop","font.color=FFFFFF","--prop","bold=true","--json"],"reason":"Format the workbook header"}
```

```json
{"operation":"set","path":"budget.xlsx","arguments":["/预算","--prop","freeze=A2","--json"],"reason":"Freeze the first worksheet row"}
```

Add a column chart using category and Q1-total ranges that already exist:

```json
{"operation":"add","path":"budget.xlsx","arguments":["/预算","--type","chart","--prop","chartType=column","--prop","dataRange=预算!E2:E4","--prop","categories=预算!A2:A4","--prop","title=Q1 合计","--prop","anchor=G2:N18","--json"],"reason":"Add the category Q1 total chart"}
```

Read back the relevant range, including exact formula text and formats:

```json
{"operation":"get","path":"budget.xlsx","arguments":["/预算/A1:E4","--depth","1","--json"]}
```

For the first save-as edit, keep `path` as the frozen source and supply a distinct
`destinationPath`. Apply later mutations to `budget-q1.xlsx` itself so earlier changes are
preserved:

```json
{"operation":"set","path":"budget-template.xlsx","destinationPath":"budget-q1.xlsx","arguments":["/预算/E2","--prop","formula=SUM(B2:D2)","--json"],"reason":"Create the Q1 workbook without overwriting the template"}
```

Render the populated region to a workspace PNG:

```json
{"operation":"view","path":"budget.xlsx","arguments":["screenshot","--range","预算!A1:N18","--json"],"outputPath":"budget-preview.png","reason":"Render the workbook for visual inspection"}
```

Validate the OOXML package and formula references:

```json
{"operation":"validate","path":"budget.xlsx","arguments":["--json"]}
```

## Create

1. Confirm sheet names, headers, data types, formulas, formats, filters, frozen panes, and chart requirements.
2. Create a real `.xlsx` through `office_spreadsheet`. Keep numeric and date values typed; do not store them as formatted strings.
3. Write formulas into formula cells. Use absolute and relative references deliberately and keep ranges aligned with the data.
4. Apply number formats separately from values. Use reusable styles consistently across headers, totals, inputs, and outputs.
5. Add charts only after the source range is stable, and verify category and series references.

## Edit

1. Inspect relevant sheets, formulas, named ranges, tables, merged cells, and charts before mutation.
2. Preserve unrelated sheets, formulas, styles, workbook metadata, and embedded objects.
3. Prefer targeted range or object changes over reconstructing the workbook.
4. Use `destinationPath` to save to a new workbook unless in-place editing was explicitly requested.

## Verification

- Confirm required sheets and dimensions, representative cell values and types, exact formula text, number formats, frozen panes, and chart ranges.
- Check formula errors and validation warnings reported by the tool. Do not infer calculated values when the engine did not calculate them.
- Render the relevant sheet or range and inspect clipping, column widths, hidden content, chart labels, and totals.
- Treat a successful engine exit as necessary but insufficient when formulas or layout are material.
- Report only checks that actually ran and preserve warnings in the final result.
