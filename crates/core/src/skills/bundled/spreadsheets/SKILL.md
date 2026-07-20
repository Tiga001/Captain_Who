---
name: spreadsheets
description: Create, edit, inspect, calculate, render, and validate Microsoft Excel-compatible .xlsx workbooks with the native office_spreadsheet tool. Use for formulas, formatting, tables, charts, multiple sheets, or other spreadsheet work.
---

# Spreadsheets

Use `office_spreadsheet` for Excel workbook work. Do not invoke OfficeCLI through a shell command or install ad hoc Python packages.

## Workflow

1. Call the tool's `status` operation before the first spreadsheet operation in a run. Use `help` when the required operation or parameters are uncertain.
2. Inspect the workbook, sheets, ranges, formulas, and charts before editing an existing file.
3. Make structured, bounded changes inside the active workspace. Use formulas where the user requests calculated values.
4. Recalculate or validate as supported, then render relevant sheets or ranges when visual presentation matters.
5. Report the output path, formulas or structural changes made, and the checks actually performed.

If `status` reports that the Office engine is unavailable, return that error faithfully and explain which capability is missing. Never replace a requested workbook with CSV, fabricate cached formula results, or claim success without a successful tool result and output verification.

Read [references/workflows.md](references/workflows.md) for detailed workbook creation, editing, formula, chart, and verification guidance when performing a spreadsheet task.
