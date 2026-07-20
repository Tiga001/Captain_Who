---
name: spreadsheets
description: Create, edit, inspect, calculate, render, and validate Microsoft Excel-compatible .xlsx workbooks with native structured operations or reproducible Python and Node.js scripts. Use for formulas, formatting, tables, charts, multiple sheets, data-driven generation, batch changes, or other spreadsheet work.
---

# Spreadsheets

Choose the execution path that matches the task:

- Use `office_spreadsheet` for inspection and small, targeted, structured changes. It provides the strongest in-place safety and should remain the default when the provider supports the requested operation directly.
- For complex, repetitive, data-driven, batch, or reproducible workbook work, create or update a saved `.py` or `.mjs` generator with `apply_patch` or `write_file`, then execute that file with `run_command` and the managed Artifact Runtime.
- A hybrid workflow is valid: generate or transform with a script, then inspect formulas and ranges, render, and validate the resulting `.xlsx` with the native tools.

Never invoke OfficeCLI itself through a shell command, and never use a system-PATH Python or Node.js executable for this script route. If the managed Artifact Runtime or a declared dependency is unavailable, return that preflight failure faithfully. Do not use inline Python or JavaScript, heredocs, shell redirection, or shell text utilities to bypass the normal file-editing tools. Editing the script and executing the script remain separate, independently authorized tool calls; runtime selection and artifact observation do not grant permission. Never invent success or silently install packages.

## Workflow

1. Inspect an existing workbook before editing it, and choose the native or scripted path deliberately. Prefer a distinct output file unless the user explicitly requested an in-place edit.
2. Before the first native spreadsheet operation in a run, call `office_spreadsheet` with `status`. Use `help` when the required operation or parameters are uncertain.
3. For a scripted operation, keep the generator as a reviewable `.py` or `.mjs` file and name that saved script explicitly in the command. Always set `observe.kinds=["office"]` and list every file that the command should create or modify in `observe.expectedOutputs`. Use `observe.additionalRoots` only when changes to other Office files in a directory also need to be detected; an external directory scan requires `read=all`, and an expected external output does not require scanning its parent.
4. Inspect `artifactObservation` on every command result, including non-zero exits, timeouts, and cancellations. A zero exit code is not proof of workbook success. Confirm that every expected workbook was created or changed, and disclose unexpected replacements, deletions, renames, partial coverage, or failed observation. Never retry blindly after a command that produced file effects.
5. Verify formulas, types, formats, sheets, and charts as applicable. Recalculate or validate as supported, then render relevant sheets or ranges when visual presentation matters.
6. Report the output path, observed file effects, formulas or structural changes made, and the checks actually performed.

If native `status` reports that the Office engine is unavailable, preserve that error and do not claim that native inspection, rendering, or validation ran. A script route may continue only when the managed runtime is available and its output can be independently observed and verified; disclose any missing native checks. Never replace a requested workbook with CSV, fabricate cached formula results, or claim success without a successful command result, matching artifact effects, and output verification.

Read [references/workflows.md](references/workflows.md) for detailed workbook creation, editing, formula, chart, and verification guidance when performing a spreadsheet task.
