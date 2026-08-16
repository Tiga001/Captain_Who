# Word document workflows

## Contents

- [Route the task](#route-the-task)
- [Native read and verification contract](#native-read-and-verification-contract)
- [Prepare and run one Managed Builder](#prepare-and-run-one-managed-builder)
- [Use exceptional Python fallback](#use-exceptional-python-fallback)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Convert once and follow the PDF Skill](#convert-once-and-follow-the-pdf-skill)
- [Generate, verify, and iterate](#generate-verify-and-iterate)
- [Clean up exact task artifacts](#clean-up-exact-task-artifacts)
- [Word quality checks](#word-quality-checks)

## Route the task

Use `office_document` only for `inspect` and `render`. Create every new document with the Managed
Builder. Edit every existing document with the fixed Managed Editor in
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

## Use exceptional Python fallback

Use a task-scoped self-authored Python script only when an unexpected limitation prevents the
fixed Builder or Editor from creating or editing the requested `.docx`. Keep it in the task's
existing temporary script directory and run it as one direct `python <script>.py ...`
`run_command` with `runtimeProfile: "documents"`. Full Python, `python-docx`, and feature-specific
OOXML are allowed.

Bind non-workspace inputs through `run_command.inputs`; never open attachment or Host-private paths
directly. For an edit, freeze one mounted source and save to a distinct `.docx`. Because a
self-authored script has no template materialization receipt, Host-private candidate validation and
publication do not apply. Within the script, save to a task-temporary `.docx`, reopen and
sanity-check it, then use `os.replace` to publish the declared save-as output atomically. After the
command reaches terminal success, require the expected `artifactObservation`, inspect the output,
and complete PDF visual QA. The fallback does not bypass command permissions and is not an
alternate visual-QA converter: never use ReportLab reconstruction, page-image stitching, or another
model-authored DOCX-to-PDF script as evidence of the Word layout.

```json
{
  "command": "python word-work/custom_edit.py --source source.docx --output outputs/report-edited.docx",
  "cwd": ".",
  "runtimeProfile": "documents",
  "inputs": [
    {
      "mountPath": "source.docx",
      "path": "@attachments/<attachment-id>/source.docx"
    }
  ],
  "reason": "Run the task-scoped fallback to edit the Word document"
}
```

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

## Convert once and follow the PDF Skill

After the final `.docx` passes publication and structural inspection, create a `.pdf` destination
inside the task-owned temporary directory and call `office_document.render` exactly once:

```json
{
  "operation": "render",
  "filePath": "outputs/report.docx",
  "outputPath": "word-work/report-visual-qa.pdf",
  "outputFormat": "pdf",
  "reason": "Convert the final Word report to PDF for complete visual review"
}
```

The request must contain all five fields shown above; `outputPath` must end in `.pdf`. Accept only a
successful result containing a PDF output with an exact readable `outputs[].readPath`, its nested
`outputs[].pageCount`, the frozen DOCX identity `outputs[].sourceSha256`, and the managed runtime
identity `outputs[].rendererRevision`. Bind the visual evidence ledger to `sourceSha256`; a changed
DOCX can never reuse an older PDF or ledger. Do not reconstruct the PDF path from `outputPath`,
stdout, a filename, or a directory scan.

Then activate and follow the PDF Skill. This handoff is an instruction, not a runtime-enforced
state transition. Bind the exact PDF `readPath`, use `pdfinfo` to obtain authoritative page count
`N`, and fail verification if it disagrees with that PDF output's nested `pageCount`. Render all
pages `1..N` with `pdftoppm` in contiguous batches of at most 32 pages. For each batch, consume
every exact returned page-image path with `read_image.path` before cleanup, then record one visual
verdict for every page in the document-wide ledger. A contact sheet, guessed range, file size, or
partial sample never proves complete coverage.

Check clipping, overflow, empty pages, tables across page boundaries, image placement, font
substitution, headers, and footers. If `PAGE` or `NUMPAGES` fields exist, confirm that they render
visibly and follow the document's numbering rules; `NUMPAGES` must agree with authoritative `N`.
Do not use ReportLab, image stitching, or a self-authored converter. If managed conversion is
unavailable, report visual QA as incomplete instead of manufacturing evidence.

## Generate, verify, and iterate

1. For creation, materialize one Builder and generate one new `.docx`. For an existing file, follow
   [editing-existing.md](editing-existing.md) instead.
2. Wait for terminal command success and confirm the expected `artifactObservation` effect. A
   successful terminal result means the Host's pinned OfficeCLI schema gate accepted the private
   candidate before atomic publication. Do not call native `validate`.
3. Inspect headings, body order, tables, sections, headers/footers, media, and requested content.
4. Run explicit structural checks for comments, tracked changes, fields, content controls, or other
   features whose semantics matter; rendering cannot verify them.
5. Convert the final DOCX once, follow the PDF Skill, and read every page `1..N` into a numbered
   ledger.
6. If a defect exists, patch the same script. Any DOCX change invalidates the PDF, `N`, page images,
   and ledger; delete stale evidence and repeat conversion plus all affected checks.
7. Clean up exact task-owned artifacts only after verification.

On a prepublish schema failure, use the bounded Host diagnostics: the stable OfficeCLI `type`,
`description`, `path`, `part`, `code`, `error`, and `message` fields plus the Host error code/message
when present. Patch the same script from those details; do not retry unchanged or expose provider
arguments. A schema gate does not prove semantic fidelity or visual quality. When PDF conversion,
rendering, or image reading is unavailable, report the affected coverage as unverified rather than
inventing a verdict.

## Clean up exact task artifacts

Keep the temporary PDF and every returned page-image `readPath` until every page has been read and
the ledger is complete. Then delete the exact task-created Builder, Editor or fallback script, QA
PDF, any workspace page images, and other task-created workspace files. PDF Skill page images are
Host-managed Artifacts, not workspace files; do not search for, copy, or delete their private
physical files, and let the Host clean that managed Run workspace. Remove the task directory with
`rmdir` only when this task created it and it is empty. Preserve the final `.docx`, pre-existing
directories, and unrelated files; never use recursive deletion. Do not present the QA PDF as a
final artifact unless the user explicitly requested it.

## Word quality checks

- Use real heading styles, lists, tables, page breaks, headers, footers, and page fields.
- Keep images proportional and inside margins; add useful alt text when supported.
- Check table widths, cell content, page breaks, orphaned headings, and accidental empty pages.
- During edits, preserve sections, relationships, media, styles, and unrelated content.
- Treat comments, tracked changes, fields, content controls, numbering, and section linkage as
  structural features. Visual rendering alone cannot prove preservation.
- Verify rendered `PAGE` and `NUMPAGES` fields against the PDF's authoritative page count and the
  document's section-numbering rules.
- Reopen or inspect the final package and report only checks that actually ran.
