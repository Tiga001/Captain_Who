---
name: spreadsheets
description: Create, edit, inspect, calculate, render, and validate Microsoft Excel-compatible .xlsx workbooks. Use for formulas, typed data, formatting, tables, charts, images, multiple sheets, repeated or data-driven generation, and any other spreadsheet task.
---

# Spreadsheets

Route by intent instead of treating every workbook write as equivalent:

- **Read:** use the flat semantic `office_spreadsheet` tool to inspect, render, and validate.
- **Create:** prefer native semantic operations for bounded work; use one saved Managed Builder for large ranges, repeated formulas or styles, coordinated multi-sheet work, imports, or bulk generation.
- **Edit an existing `.xlsx`:** inspect first. Use native semantic operations for supported bounded edits. The current Python Builder can load one explicit `--source` workbook with `openpyxl`, but it is a general Builder, not a fixed Editor or Host-owned typed edit transaction. Use that route only for an explicit, fidelity-controllable transformation, default to a distinct save-as output, and fail closed when preservation of an existing feature cannot be established. Never clear, reconstruct, or round-trip an existing workbook merely to imitate an edit.
- Combine paths only at their intended boundaries: native inspect/validate/render around one Builder run.

Each native mutation is an independent atomic file transaction, not a multi-operation batch. Re-inspect after sheet removal or movement before issuing another operation whose target depends on sheet order.

Never expose OfficeCLI arguments, workbook DOM paths, executable paths, runtime versions, or package versions to the model-facing call. Every native call uses flat top-level semantic fields plus a required `reason`; never wrap it in `request`. Keep `reason` to one non-empty user-facing sentence of at most 240 characters. When the native tool returns `capabilityNotSupported` with `recovery=useManagedScript`, use the Builder for creation. For an existing workbook, switch only when a targeted `--source` transformation can meet the preservation contract; otherwise fail closed. Never guess low-level fields or repeat the failed call.

## Managed Builder

Before materializing, choose one dedicated workspace-relative script directory for this run. Reuse an existing plain directory when appropriate; otherwise create it first with a separate idempotent `mkdir -p <script-directory>` `run_command`. Use that same directory for materialization, patching, and execution.

Use `skills_list_resources` to locate `templates/builder.py`, ensure the selected script directory exists, then materialize it once with `skills_materialize_resource` into a new path inside that directory. Patch and rerun that same Builder; do not create a trail of replacement scripts.

After every materialization or patch, rely on the Host's automatic Python syntax preflight for the exact saved Builder selected by the trusted materialization receipt. Do not issue a model-authored syntax-check command and never request system Python, `python -m`, `python -c`, inline code, heredocs, or a command combined with `&&`, `|`, or `;`. Any Builder change invalidates the prior preflight. If syntax preflight fails, no Builder execution is allowed: patch the same file and submit it again for a fresh Host check.

Execute that materialized file with `run_command` and a direct logical `python <builder>.py --output <file.xlsx>` command. Omit `runtimeProfile` and `observe`: the host verifies this run's materialization receipt, derives the `spreadsheets` profile from the static Office output, binds the pinned runtime, and observes that output automatically. Never use system Python, `pip`, `npm`, inline code, heredocs, or shell redirection.

Bind source workbooks, CSVs, images, attachments, and earlier generated files through `run_command.inputs`:

```json
{
  "mountPath": "source/template.xlsx",
  "path": "inputs/template.xlsx"
}
```

Use the exact path returned by the producing tool or supplied by the user. The Host automatically recognizes workspace, absolute/system, `@attachments/...`, `image-artifact://...`, and revision-bound `skill://...` paths. `mountPath` is optional and defaults to the source filename. Scripts read only the host-mounted path below `MYCOPILOT_INPUT_ROOT`; never pass or open an `@attachments` or `skill://` URI directly.

Every Builder command must declare its generated workbook with exactly one static `--output` argument. Inspect the backend-owned `artifactObservation` even after failure, timeout, or cancellation. Never blindly rerun a command that may have changed files.

For an existing workbook, bind exactly one source, pass its logical mount path through `--source`, and publish to a distinct `.xlsx` output unless the user explicitly requires in-place behavior. Preserve unrelated sheets, formulas, styles, defined names, charts, links, and media. If macros, signatures, external connections, pivots/slicers, advanced drawings, or another feature cannot be preserved and verified by the selected route, stop with the limitation instead of silently rebuilding or degrading it.

`openpyxl` stores formula text but does not calculate formulas. Native package validation and exact formula inspection do not prove evaluated results, and a source workbook's cached values may be stale. Claim recalculation only when an authoritative calculation engine returned explicit evidence; otherwise disclose that Excel must recalculate the formulas when opened.

## Completion gate

1. Inspect the source before an edit. Use only a fidelity-controllable route and prefer a distinct save-as output; never rebuild an existing workbook to imitate an edit.
2. Generate or edit the workbook while keeping numbers, dates, booleans, and formulas typed.
3. Confirm the expected file effect in the native result or `artifactObservation`.
4. Inspect the final sheet list, each sheet's complete populated used range, and every reported chart or image anchor; inspect representative typed values, exact formula text, formats, tables, and chart ranges, then validate the final package.
5. Build a per-sheet visual ledger from that final inspect result. For each sheet, render the visual extent formed by the populated used range plus every reported floating chart or image bound, consume its exact returned `outputs[].readPath` with `read_image.path`, and record one verdict. If a floating object's bounds cannot be established, disclose incomplete visual coverage instead of claiming a full-sheet verdict. `pageSelection` records a request, not coverage. Any later workbook edit invalidates the entire ledger.
6. Keep syntax, execution, package, calculation, and visual evidence separate. Report only checks that actually succeeded; preserve structured errors and disclose unavailable calculation or visual verification.
7. Before the final response, delete the exact task-owned Builder and temporary files. If this task created the script directory and it is empty, remove it with `rmdir`; preserve pre-existing directories and unrelated files, never use recursive deletion, and keep the Builder only when the user explicitly requests it.

Read [references/workflows.md](references/workflows.md) for exact semantic and Builder examples, input binding, verification, and spreadsheet-specific quality checks.
