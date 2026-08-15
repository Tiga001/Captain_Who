# Word document workflows

## Contents

- [Route the task](#route-the-task)
- [Native read and verification contract](#native-read-and-verification-contract)
- [Prepare and run one Managed Builder](#prepare-and-run-one-managed-builder)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Consume render outputs](#consume-render-outputs)
- [Generate, verify, and iterate](#generate-verify-and-iterate)
- [Word quality checks](#word-quality-checks)

## Route the task

Use `office_document` only for `status`, `inspect`, `validate`, and `render`. Create every new
document with the Managed Builder. Edit every existing document with the fixed Managed Editor in
[editing-existing.md](editing-existing.md). Do not use a native create/mutation sequence, rebuild an
existing file with the Builder, or unzip and patch OOXML as a generic fallback.

## Native read and verification contract

Call `office_document` with a flat semantic object:

```json
{
  "operation": "inspect",
  "filePath": "outputs/report.docx",
  "reason": "Inspect the Word report structure before verification"
}
```

Keep `operation`, its semantic fields, and `reason` at the root. Never add a `request` wrapper,
provider arguments, an executable, format tokens, DOM paths, or shell flags. `reason` is required
user-visible audit text only; it never grants permission. Copy exact returned output paths and
structural facts rather than inventing them.

## Prepare and run one Managed Builder

The bundled `templates/builder.py` is a compact `python-docx` starting point for new documents.
Locate its exact revision-bound URI with `skills_list_resources`. Choose a workspace-relative
script directory first. Reuse it if it already exists as a plain directory; otherwise create it
with a separate idempotent command:

```json
{
  "command": "mkdir -p scripts",
  "cwd": ".",
  "reason": "Prepare a workspace directory for the Word Builder"
}
```

Materialize the Builder once into a new path:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/builder.py",
  "destination": "scripts/build_report.py",
  "reason": "Create a reviewable Word builder from the activated Skill template"
}
```

The URI is illustrative; use the exact value returned by the active Skill. Patch and rerun the same
Builder. Do not rematerialize over it or create replacement scripts for corrections.

The Host automatically performs Python syntax preflight before execution. A patch invalidates the
previous check. When preflight fails, patch the same Builder and submit the normal build command
again. Do not run a model-authored syntax check, system Python, `python -m`, `python -c`, inline
code, heredocs, or a compound shell command.

Run the materialized template with a direct
`python scripts/build_report.py --output outputs/report.docx` command. Omit `runtimeProfile` and
`observe`: the Host verifies the materialization receipt, binds the pinned `documents` runtime,
freezes the run, directs output through a private candidate, and observes the declared destination.
Every Builder build command must declare exactly one static `--output` ending in `.docx`.

After final verification, delete the exact Builder and task-created temporary files. If this task
created the script directory and it is empty, remove it with `rmdir`. Preserve pre-existing
directories and unrelated files; never use recursive deletion.

## Bind inputs declaratively

Every non-workspace input used by a Builder or Editor must appear in `run_command.inputs`:

```json
{
  "command": "python scripts/build_report.py --output outputs/report.docx --image images/campus.jpg",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "images/campus.jpg",
      "path": "@attachments/<attachment-id>/campus.jpg"
    }
  ],
  "reason": "Build the illustrated Word report and track its output"
}
```

Supported source references include authorized workspace or absolute paths, exact attachment
`readPath` values, exact generated artifact URIs, and exact revision-bound Skill URIs. `mountPath`
is a logical private-input-root-relative name. The Host freezes each source and exposes only the
run-scoped `MYCOPILOT_INPUT_ROOT`; scripts call their fixed `mounted_input` helper. Never open a
source URI or attachment library path directly.

## Observe every file effect

The Host derives `observe.expectedOutputs` from the one static output and automatically binds
Office observation. Use `observe.additionalRoots` only when a separately authorized task truly
needs to discover another Office effect; never scan a broad external directory.

Inspect `artifactObservation` after success, non-zero exit, timeout, and cancellation:

- match the destination in `artifactObservation.expectedOutputs`;
- accept only a terminal `created`, `modified`, `replaced`, or `renamed` effect as publication
  evidence;
- treat `missing`, `unchanged`, `unobserved`, `invalid`, incomplete coverage, or failed observation
  as insufficient;
- report unexpected creations, replacements, deletions, or renames;
- never blindly rerun a command that may have changed files.

When a command returns `status: "running"`, wait through its returned `continueWith` receipt. A
running result is not terminal publication evidence.

## Consume render outputs

The current semantic surface does not expose an authoritative rendered page count. Render the
whole document once for overview, then render each known or affected page individually:

```json
{
  "operation": "render",
  "filePath": "outputs/report.docx",
  "pageOrSlide": 1,
  "outputPath": "outputs/report-page-1.png",
  "reason": "Render page 1 of the final Word report for visual review"
}
```

For each successful render, select the output whose `role` is `render` and `kind` is `image`, then
pass only its exact `outputs[].readPath` to `read_image.path`. Never reconstruct a path from the
request, stdout, filename, or directory search. Keep a numbered ledger of pages actually read.

`pageSelection` records the requested selection, not the document's actual page count. A
whole-document contact sheet is overview-only and does not prove count, coverage, or small-text
legibility. Disclose that exhaustive coverage is unavailable unless a future trusted output
explicitly reports the final count.

## Generate, verify, and iterate

1. For creation, materialize one Builder and generate one new `.docx`. For an existing file, follow
   [editing-existing.md](editing-existing.md) instead.
2. Wait for terminal command success and confirm the expected `artifactObservation` effect.
3. Inspect headings, body order, tables, sections, headers/footers, media, and requested content.
4. Run native `validate`.
5. Run explicit structural checks for comments, tracked changes, fields, content controls, or other
   features whose semantics matter; rendering cannot verify them.
6. Render and read the overview plus every known or affected page separately. Check clipping,
   overflow, blank pages, broken images, font substitution, hierarchy, and spacing.
7. If a defect exists, patch the same script. Discard stale evidence and repeat all affected checks
   on the new final file.
8. Clean up the task-owned script and temporary files only after verification.

Do not claim visual quality from package validation alone. When rendering or image reading is
unavailable, report the affected coverage as unverified rather than inventing a verdict.

## Word quality checks

- Use real heading styles, lists, tables, page breaks, headers, footers, and page fields.
- Keep images proportional and inside margins; add useful alt text when supported.
- Check table widths, cell content, page breaks, orphaned headings, and accidental empty pages.
- During edits, preserve sections, relationships, media, styles, and unrelated content.
- Treat comments, tracked changes, fields, content controls, numbering, and section linkage as
  structural features. Visual rendering alone cannot prove preservation.
- Reopen or inspect the final package and report only checks that actually ran.
