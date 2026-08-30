# Editing an existing workbook

Use the fixed Python Editor for every existing `.xlsx` write. The Editor executes
normal Python in the pinned managed spreadsheet runtime; `openpyxl` remains the workbook
authoring and round-trip engine. This is not an AST, JSON, or operation DSL. The model may
use helper functions, loops, conditions, comprehensions, and pinned library imports inside
`edit_workbook`.

## Workflow

1. Inspect the source workbook. Record sheets, used ranges, formulas, tables, charts,
   images, defined names, validations, formatting, and preservation-sensitive features.
2. Choose one task-owned script directory. Reuse an existing plain directory or create
   it first with a separate `mkdir -p <directory>` command. Record whether the task
   created it.
3. Locate `templates/editor.py` with `skills_list_resources`, then materialize that
   exact revision once into a new path in the prepared directory.
4. Patch only `edit_workbook`. Keep the CLI, mounted-input resolver, direct candidate save,
   and `BEGIN/END EDIT REGION` markers unchanged. Use normal Python rather than encoding
   the task as JSON or a list of artificial operations.
5. Bind exactly one source and declare one distinct save-as output:

```json
{
  "command": "python workbook-work/edit_workbook.py --source source.xlsx --output workbook-edited.xlsx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source.xlsx",
      "path": "workbook.xlsx"
    }
  ],
  "reason": "Edit the existing workbook and publish a Host-gated save-as copy"
}
```

Omit `runtimeProfile` and `observe`. The exact bundled-resource receipt selects the
managed spreadsheet runtime and automatic syntax preflight. If the result is `running`,
wait on the same command session. Do not inspect or link the output until its terminal
result succeeds and `artifactObservation` confirms the expected workbook.

The Host freezes the script bytes, runtime, source snapshot, declared inputs, and target
precondition before execution. It runs the frozen Python script under the existing
`run_command` permission and approval boundary, redirects the intended output to private
staging, reopens that candidate with pinned `openpyxl.load_workbook(data_only=False)`, rechecks
source/target/cancellation state, and only then publishes atomically. The Host owns this gate; do
not duplicate the candidate reopen in the script or call native `validate`. This guarantees the
declared Office output transaction; it does not pretend unrestricted Python is a separate
cross-platform OS sandbox.

6. Require terminal success from the Host openpyxl reopen gate. Re-inspect the final workbook and
   compare the intended values, formula text, styles, sheets, tables, charts, images, and names.
   Rebuild the per-sheet visual ledger from the final revision; do not call native `validate`.
7. Delete the exact task-owned Editor and temporary files. Use `rmdir` only when this
   task created the now-empty directory. Never recursively delete it.

## Authoring and preservation rules

- Load and save only through the fixed wrapper. Do not reopen the physical source path or
  bypass the declared output.
- Keep values typed. Store formulas as formulas, never as displayed numbers.
- Use `input_root / <declared mountPath>` for replacement images or data files.
- Preserve unrelated sheets, formulas, styles, defined names, tables, charts, links,
  validations, and media. Do not clear or reconstruct a workbook to imitate an edit.
- `openpyxl` stores formula expressions but does not calculate them. Claim recalculation
  only when a separate authoritative calculation engine returned explicit evidence.
- Stop rather than silently degrade macros or signatures, external connections, Power
  Query, pivots or slicers, advanced drawings, embedded objects, or any feature whose
  round-trip fidelity cannot be established and verified.
