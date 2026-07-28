# Word document workflows

## Contents

- [Route the task](#route-the-task)
- [Native semantic contract](#native-semantic-contract)
- [Create and reuse one Builder](#create-and-reuse-one-builder)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Consume render outputs](#consume-render-outputs)
- [Generate, verify, render, iterate](#generate-verify-render-iterate)
- [Word quality checks](#word-quality-checks)

## Route the task

Use `office_document` first when the request is a bounded combination of its semantic operations:

`create`, `inspect`, `validate`, `render`, `addText`, `insertImage`, `addTable`, `addHeader`,
`addFooter`, `replaceText`, `formatText`, `removeBlock`, and `moveBlock`.

Use the Managed Builder when the task needs coordinated page design, many repeated sections,
advanced OOXML features, mail-merge-like generation, large batches, or a semantic operation that
the backend reports as unsupported. Do not emulate an unsupported operation with low-level
OfficeCLI fields. A hybrid flow—Builder write, native inspect/validate/render—is usually best for
complex deliverables.

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

File and image inputs use the shared `AgentFileInputRef` object rather than guessed paths. For an
attachment, call `attachments_list` and copy its exact `readPath`. For a generated image or earlier
image-generation result, use a `generated_artifact` reference. If the backend returns
`office.capability_not_supported`, `capabilityNotSupported`, or
`recovery=useManagedScript`, preserve the error and switch to the Builder path. Do not repeat the
same failed call with invented fields.

Insert a registered attachment with its typed source reference:

```json
{
  "operation": "insertImage",
  "filePath": "outputs/report.docx",
  "source": {
    "type": "attachment",
    "readPath": "@attachments/<attachment-id>/campus.jpg"
  },
  "altText": "Campus overview",
  "width": "6in",
  "reason": "Insert the supplied campus image into the Word report"
}
```

Inspect an existing document before mutation. Prefer save-as for transformations unless the user
explicitly requests in-place editing. Use the semantic result's canonical block identity for later
targeted operations instead of inventing positional identities.

## Create and reuse one Builder

The bundled `templates/builder.py` is a compact `python-docx` starting point. Locate its exact
revision-bound URI with `skills_list_resources`, then copy it once into a new workspace path:

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

The host-owned `documents` profile pins:

- Python 3.12.13 with `python-docx` 1.2.0.
- Node.js 22.23.1 with `docx` 9.6.1.

These versions describe the immutable profile; never send or install them. Run the materialized
template with a direct logical `python <script>.py --output <file.docx>` command. The Host verifies
the run-scoped materialization receipt, derives and freezes the `documents` profile from the static
Office output, and binds observation; omit `runtimeProfile` and `observe`. Never call a private
executable path, system Python/Node.js, `pip`, `npm`, inline code, a heredoc, or shell redirection.

## Bind inputs declaratively

Every input needed by a Builder must be explicit in `run_command.inputs`:

```json
{
  "command": "python scripts/build_report.py --output outputs/report.docx --image images/campus.jpg",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "images/campus.jpg",
      "source": {
        "type": "attachment",
        "readPath": "@attachments/<attachment-id>/campus.jpg"
      }
    }
  ],
  "reason": "Build the illustrated Word report and track its output"
}
```

Supported source types are:

- `attachment`: exact registered `readPath`.
- `workspace`: workspace file `path`.
- `external`: authorized external file `path`.
- `generated_artifact`: exact image Artifact `uri` plus the exact absolute `savedPath` as `path`.
- `skill_resource`: exact revision-bound `uri`.

`mountPath` is a private input-root-relative filename. The Host freezes and revalidates the input,
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

Render to an explicit review image:

```json
{
  "operation": "render",
  "filePath": "outputs/report.docx",
  "outputPath": "outputs/report-preview.png",
  "reason": "Render every final page for visual review"
}
```

After a successful render, take the exact path from `outputs[].readPath` and pass it as
`read_image.path`:

```json
{
  "path": "outputs/report-preview.png"
}
```

Treat the returned output as authoritative:

- Select the output whose `role` is `render` and whose `kind` is `image`.
- Use only its `readPath`; never reconstruct a path from the render request, `source`, `argv`,
  `cwd`, `stdout`, or a file search.
- Never rerender merely to discover where the first render was published.
- `pageSelection` records the requested selection (`all` or explicit page numbers); it is not an
  independent proof of the document's actual page count. Establish the final page count with
  `inspect`, compare it with the request, and then inspect the returned image.
- If the output is absent, not readable, or `read_image` reports an unsupported model capability,
  state that visual verification was unavailable.
- If a preview exceeds the visual-input limit, render bounded page groups rather than repeating the
  same oversized request. Do not claim inspection of unread images.

## Generate, verify, render, iterate

Use this fixed loop for a final document:

1. Generate or edit the `.docx`.
2. Confirm the expected file effect.
3. Inspect headings, body order, tables, headers, footers, media, and required content.
4. Run native `validate`.
5. Run one native `render` request covering all final pages with a contact-sheet grid.
6. Read the exact returned render output with `read_image`, then visually inspect every rendered
   page for clipping, overflow, blank pages, broken images, font substitution, weak hierarchy, and
   inconsistent spacing.
7. If a defect exists, patch the same Builder or issue one corrected semantic operation, regenerate,
   and repeat validation and rendering.

Do not claim visual quality from package validation alone. If rendering returns an
`office.render_backend_*` error, report that visual verification was unavailable; do not launch a
user browser or render every page in separate retry calls.

## Word quality checks

- Use real heading styles, lists, tables, page breaks, headers, footers, and page fields.
- Preserve sections, relationships, media, styles, and unrelated content during edits.
- Keep images proportional and within page margins; add useful alt text when supported.
- Check table widths, cell content, page breaks, orphaned headings, and accidental empty pages.
- Reopen or inspect the final package and report only checks that actually ran.
