# Word document workflows

## Contents

- [Route the task](#route-the-task)
- [Native semantic contract](#native-semantic-contract)
- [Prepare, create, and reuse one Builder](#prepare-create-and-reuse-one-builder)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Consume render outputs](#consume-render-outputs)
- [Generate, verify, render, iterate](#generate-verify-render-iterate)
- [Word quality checks](#word-quality-checks)

## Route the task

Use `office_document` for reads, validation, rendering, and bounded combinations of its semantic
write operations:

`create`, `inspect`, `validate`, `render`, `addText`, `insertImage`, `addTable`, `addHeader`,
`addFooter`, `replaceText`, `formatText`, `removeBlock`, and `moveBlock`.

Use the Managed Builder only to create a new document that needs coordinated page design, many
repeated sections, advanced generation features, mail-merge-like generation, or large batches. It
is not a fidelity-preserving Editor for an existing `.docx`.

For an existing document, inspect first and use only supported native edit operations. If the
requested change is outside that surface, preserve the source and report the operation as
unsupported until a fixed Editor is available. Do not rebuild the document, unzip or patch OOXML,
or use low-level OfficeCLI fields to imitate an edit.

## Native semantic contract

Call `office_document` with a flat object:

```json
{
  "operation": "create",
  "filePath": "outputs/report.docx",
  "reason": "Create the requested Word report"
}
```

Keep `operation`, its semantic fields, and `reason` at the root. Never add a `request` wrapper,
provider arguments, an executable, format tokens, DOM paths, or shell flags. `reason` is required,
user-visible audit text only; it never grants permission.

File and image inputs use one path string. For an attachment, call `attachments_list` and copy its
exact `readPath`; for a generated image, copy the exact `image-artifact://...` path. If the backend returns
`office.capability_not_supported`, `capabilityNotSupported`, or
`recovery=useManagedScript`, preserve the error. Switch to the Builder only when creating a new
document; fail closed for an existing-document edit. Do not repeat the same failed call with
invented fields.

Insert a registered attachment with its exact path:

```json
{
  "operation": "insertImage",
  "filePath": "outputs/report.docx",
  "imagePath": "@attachments/<attachment-id>/campus.jpg",
  "altText": "Campus overview",
  "width": "6in",
  "reason": "Insert the supplied campus image into the Word report"
}
```

Inspect an existing document before mutation. Prefer save-as for transformations unless the user
explicitly requests in-place editing. Copy the exact `blockIndex` from a fresh inspect result for a
targeted operation instead of inventing an index. `removeBlock` and `moveBlock` can shift later
indices, so re-inspect before another targeted operation. Each native mutation is an independent
atomic file transaction; several calls are not one all-or-nothing batch for the whole request. A
`capabilityNotSupported`/`useManagedScript` response may route a new-document request to the
Builder, but it must fail closed for an existing-document edit.

## Prepare, create, and reuse one Builder

The bundled `templates/builder.py` is a compact `python-docx` starting point. Locate its exact
revision-bound URI with `skills_list_resources`. First select a dedicated workspace-relative
script directory. Reuse it if it is already a plain directory; otherwise create it with a separate
idempotent command:

```json
{
  "command": "mkdir -p scripts",
  "cwd": ".",
  "reason": "Prepare a workspace directory for the Word Builder"
}
```

The directory name is not fixed. Materialize the Builder once into a new path inside it:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/builder.py",
  "destination": "scripts/build_report.py",
  "reason": "Create a reviewable Word builder from the activated Skill template"
}
```

Use the exact `sourceUri` returned by the resource list; the placeholder above is not a literal
URI. `skills_materialize_resource` is create-only. After materialization, patch
`scripts/build_report.py` and rerun that same file. Do not rematerialize over a modified Builder or
create a new script for every correction.

The Host performs an isolated Python syntax preflight automatically before every Builder
execution. A materialization or patch makes that script revision require a new preflight. If the
preflight fails, execution is blocked: patch the same Builder, then submit its normal build command
again. Do not issue a model-authored syntax command, use system Python, invoke `python -m` or
`python -c`, or combine a check and build in one shell command.

The bundled Builder route pins Python 3.12.13 with `python-docx` 1.2.0.

These versions describe the immutable profile; never send or install them. Run the materialized
template with a direct logical `python <script>.py --output <file.docx>` command. The Host verifies
the run-scoped materialization receipt, derives and freezes the `documents` profile from the static
Office output, and binds observation; omit `runtimeProfile` and `observe`. Never call a private
executable path, system Python, `pip`, inline code, a heredoc, or shell redirection.

After final verification, delete the exact Builder and every task-created temporary file. If this
task created the script directory and it is empty, remove it with `rmdir`. Preserve a pre-existing
directory and every unrelated file, never use recursive deletion, and retain the Builder only when
the user explicitly requests it.

## Bind inputs declaratively

Every input needed by a Builder must be explicit in `run_command.inputs`:

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

Supported paths are workspace-relative paths, authorized absolute/system paths, exact attachment
`readPath` values, exact generated `image-artifact://...` paths, and exact revision-bound
`skill://...` URIs. The Host resolves the internal source type.

`mountPath` is an optional private input-root-relative filename and defaults to the source filename. The Host freezes and revalidates the input,
then exposes the run-scoped root in `MYCOPILOT_INPUT_ROOT`. The Builder resolves
`MYCOPILOT_INPUT_ROOT / mountPath`. Never let Python open `@attachments`, `skill://`, an attachment
library path, or another private storage path directly.

## Observe every file effect

Every Builder command must declare its expected `.docx` with exactly one static `--output`
argument. The Host automatically binds `observe.kinds=["office"]` and copies that path into
`observe.expectedOutputs`. Paths are relative to `cwd` unless workspace-external output is
authorized with `write=all`. Use an explicit `observe.additionalRoots` only to discover other
Office changes that are not already named; do not scan a whole external directory merely because
one expected output is external.

Inspect `artifactObservation` after success, non-zero exit, timeout, and cancellation:

- Match every requested file in `artifactObservation.expectedOutputs`.
- Accept `created`, `modified`, `replaced`, or `renamed` as evidence of a file effect.
- Treat `missing`, `unchanged`, `unobserved`, `invalid`, partial coverage, or failed observation as
  insufficient.
- Report unexpected creations, replacements, deletions, or renames.
- Never blindly rerun after a command that may have produced side effects.

Observation records effects; it does not grant access, make arbitrary scripts transactional, or
prove that the document is valid.

## Consume render outputs

The current semantic surface does not expose an authoritative rendered page count. First render the
whole document as an overview, then render any known or affected page to its own explicit review
image. A single-page request looks like:

```json
{
  "operation": "render",
  "filePath": "outputs/report.docx",
  "pageOrSlide": 1,
  "outputPath": "outputs/report-page-1.png",
  "reason": "Render page 1 of the final Word report for visual review"
}
```

After a successful render, take the exact path from `outputs[].readPath` and pass it as
`read_image.path`:

```json
{
  "path": "outputs/report-page-1.png"
}
```

Treat the returned output as authoritative:

- Select the output whose `role` is `render` and whose `kind` is `image`.
- Use only its `readPath`; never reconstruct a path from the render request, `source`, `argv`,
  `cwd`, `stdout`, or a file search.
- Never rerender merely to discover where the first render was published.
- `pageSelection` records only the requested selection (`all` or one explicit page); it is not an
  independent proof of the document's actual page count. Native inspect does not currently provide
  that count either.
- Keep a numbered visual ledger for every individual page image actually read. A whole-document
  contact sheet is overview-only and does not prove page count, coverage, or legibility. The current
  request schema does not support page ranges or bounded page groups.
- Unless a future trusted output explicitly returns the final rendered page count, state that
  exhaustive all-page coverage was unavailable. Never promote a visually guessed contact-sheet
  count into authoritative `1..N` evidence.
- If the output is absent, not readable, or `read_image` reports an unsupported model capability,
  state that visual verification was unavailable.
- If the overview image exceeds the visual-input limit, rely only on individual known-page renders
  that fit. Do not claim inspection of unread images or infer coverage from filenames.

## Generate, verify, render, iterate

Use this fixed loop for a final document:

1. Inspect an existing source before any supported native edit; never edit an existing `.docx`
   through the current Builder.
2. Generate a new `.docx` or apply the bounded native edit.
3. Confirm the expected file effect.
4. Inspect headings, body order, tables, headers, footers, media, and required content. Do not claim
   a final rendered page count from this structural inspection.
5. Run native `validate`.
6. When comments, tracked changes, or fields exist or are required, run an explicit structural
   check for those features. Rendering is not structural evidence; if no supported checker is
   available, report that verification as unavailable.
7. Render the whole document once for overview, then render and read every known or affected page
   separately. Record only those individually read pages in the ledger and disclose that exhaustive
   coverage is unavailable without a trusted final page count.
8. Inspect the overview and individual page evidence for clipping, overflow, blank pages, broken
   images, font substitution, weak hierarchy, and inconsistent spacing.
9. If a defect exists, patch the same Builder for a new document or issue a corrected native edit.
   Discard stale structural, validation, and visual evidence, then repeat the applicable checks on
   the new final file.

Do not claim visual quality from package validation alone. If rendering returns an
`office.render_backend_*` error, report the affected pages as unverified; do not launch a user
browser or repeat an unchanged render call.

## Word quality checks

- Use real heading styles, lists, tables, page breaks, headers, footers, and page fields.
- Preserve sections, relationships, media, styles, and unrelated content during edits.
- Treat comments, tracked changes, fields, content controls, numbering, and section linkage as
  structural features; visual rendering alone cannot verify their preservation or behavior.
- Keep images proportional and within page margins; add useful alt text when supported.
- Check table widths, cell content, page breaks, orphaned headings, and accidental empty pages.
- Reopen or inspect the final package and report only checks that actually ran; do not invent a
  rendered page count that the current tools did not return.
