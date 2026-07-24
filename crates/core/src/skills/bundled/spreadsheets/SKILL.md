---
name: spreadsheets
description: Create, edit, inspect, calculate, render, and validate Microsoft Excel-compatible .xlsx workbooks. Use for formulas, typed data, formatting, tables, charts, images, multiple sheets, repeated or data-driven generation, and any other spreadsheet task.
---

# Spreadsheets

Use one of two supported paths:

- Prefer the flat semantic `office_spreadsheet` tool for bounded, high-frequency work such as creating a workbook, adding or moving sheets, writing cells and formulas, formatting ranges, freezing panes, adding conditional formats, tables, charts or images, inspecting, rendering, and validating.
- Use one saved Managed Builder for large ranges, repeated formulas or styles, coordinated multi-sheet work, imports, advanced workbook features, bulk generation, or any capability the native tool reports as unsupported.
- Combine the paths when useful: build with a script, then inspect exact values and formulas, render, and validate with the native tool.

Never expose OfficeCLI arguments, workbook DOM paths, executable paths, runtime versions, or package versions to the model-facing call. Every native call uses flat top-level semantic fields plus a required `reason`; never wrap it in `request`. Keep `reason` to one non-empty user-facing sentence of at most 240 characters. When the native tool returns `capabilityNotSupported` with `recovery=useManagedScript`, switch once to the Builder path instead of guessing low-level fields or repeating the failed call.

## Managed Builder

Use `skills_list_resources` to locate `templates/builder.py`, then materialize it once with `skills_materialize_resource` into a new workspace path. Patch and rerun that same builder; do not create a trail of replacement scripts.

Execute that materialized file with `run_command` and a direct logical `python <builder>.py --output <file.xlsx>` command. Omit `runtimeProfile` and `observe`: the host verifies this run's materialization receipt, derives the `spreadsheets` profile from the static Office output, binds the pinned runtime, and observes that output automatically. Never use system Python, `pip`, `npm`, inline code, heredocs, or shell redirection.

Bind source workbooks, CSVs, images, attachments, and earlier generated files through `run_command.inputs`:

```json
{
  "mountPath": "source/template.xlsx",
  "source": {
    "type": "workspace",
    "path": "inputs/template.xlsx"
  }
}
```

Use the exact attachment `readPath` returned by `attachments_list`. Use `type="workspace"`, `external`, `generated_artifact`, or `skill_resource` for those corresponding sources. Scripts read only the host-mounted path below `MYCOPILOT_INPUT_ROOT`; never pass or open an `@attachments` or `skill://` URI directly.

Every Builder command must declare its generated workbook with exactly one static `--output` argument. Inspect the backend-owned `artifactObservation` even after failure, timeout, or cancellation. Never blindly rerun a command that may have changed files.

## Completion gate

1. Inspect the source before an edit and prefer a distinct output unless the user requested in-place editing.
2. Generate or edit the workbook while keeping numbers, dates, booleans, and formulas typed.
3. Confirm the expected file effect in the native result or `artifactObservation`.
4. Inspect required sheets, representative values, exact formula text, formats, tables, and chart ranges; validate the final package.
5. Render every final worksheet in one combined request when supported. On success, pass the exact returned `outputs[].source` to `read_image`; never infer a path from the request, `argv`, `stdout`, or a file search. Visually inspect clipping and chart placement, patch the same Builder or semantic request when needed, then render again.
6. Report only the file effects and checks that actually succeeded. Preserve structured errors and disclose unavailable calculation or visual verification.

Read [references/workflows.md](references/workflows.md) for exact semantic and Builder examples, input binding, verification, and spreadsheet-specific quality checks.
