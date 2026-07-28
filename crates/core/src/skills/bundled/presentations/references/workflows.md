# Presentation workflows

## Contents

- [Route the task](#route-the-task)
- [Native semantic contract](#native-semantic-contract)
- [Create and reuse one Builder](#create-and-reuse-one-builder)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Consume render outputs](#consume-render-outputs)
- [Generate, verify, render, iterate](#generate-verify-render-iterate)
- [Presentation quality checks](#presentation-quality-checks)

## Route the task

Use `office_presentation` first when the request is a bounded combination of its semantic
operations:

`create`, `inspect`, `validate`, `render`, `addSlide`, `addText`, `insertImage`, `addTable`,
`addChart`, `addShape`, `addFooter`, `removeSlide`, and `moveSlide`.

Use the Managed Builder for a complete visual narrative, repeated layouts, a coordinated theme,
complex diagrams, template population, many slides or media items, batch generation, or a semantic
operation that the backend reports as unsupported. Do not emulate an unsupported operation with
low-level OfficeCLI fields. A hybrid flow—Builder write, native inspect/validate/render—is usually
best for a complete deck.

## Native semantic contract

Call `office_presentation` with a flat object:

```json
{
  "operation": "create",
  "filePath": "outputs/product-intro.pptx",
  "reason": "Create the requested PowerPoint presentation"
}
```

Keep `operation`, its semantic fields, and `reason` at the root. Never add a `request` wrapper,
provider arguments, an executable, slide DOM paths, or shell flags. `reason` is required,
user-visible audit text only; it never grants permission.

Image, template, data, and deck inputs use one path string. For an attachment, call
`attachments_list` and copy its exact `readPath`; for a generated image, copy its exact
`image-artifact://...` path. If the backend returns
`office.capability_not_supported`, `capabilityNotSupported`, or
`recovery=useManagedScript`, preserve the error and switch to the Builder path. Do not repeat the
same failed call with invented fields.

Add a slide, then place a registered image with explicit presentation geometry:

```json
{
  "operation": "addSlide",
  "filePath": "outputs/product-intro.pptx",
  "title": "核心能力",
  "body": "Agent 对话、文件编辑、终端执行和内嵌浏览器",
  "backgroundColor": "0F172A",
  "reason": "Add the core capabilities slide"
}
```

```json
{
  "operation": "insertImage",
  "filePath": "outputs/product-intro.pptx",
  "imagePath": "@attachments/<attachment-id>/hero.png",
  "slideNumber": 2,
  "x": "7in",
  "y": "1.6in",
  "width": "5.5in",
  "height": "4.2in",
  "altText": "Product illustration",
  "reason": "Place the supplied illustration on the core capabilities slide"
}
```

Inspect an existing deck before mutation. Prefer save-as for transformations unless the user
explicitly requests in-place editing. Establish the audience, purpose, slide count, aspect ratio,
narrative, and visual direction before creating slides.

## Create and reuse one Builder

The bundled `templates/builder.mjs` is a compact `pptxgenjs` starting point with a wide layout,
theme, reusable footer, and optional mounted image. Locate its exact revision-bound URI with
`skills_list_resources`, then copy it once into a new workspace path:

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/builder.mjs",
  "destination": "scripts/build_deck.mjs",
  "reason": "Create a reviewable presentation builder from the activated Skill template"
}
```

Use the exact `sourceUri` returned by the resource list; the placeholder above is not a literal
URI. `skills_materialize_resource` is create-only. After materialization, patch
`scripts/build_deck.mjs` and rerun that same file. Do not rematerialize over a modified Builder or
create a new script for every correction.

The host-owned `presentations` profile pins:

- Node.js 22.23.1 with `pptxgenjs` 4.0.1.
- Python 3.12.13 with `python-pptx` 1.0.2.

These versions describe the immutable profile; never send or install them. Run the materialized
template with a direct logical `node <script>.mjs --output <file.pptx>` command. The Host verifies
the run-scoped materialization receipt, derives and freezes the `presentations` profile from the
static Office output, and binds observation; omit `runtimeProfile` and `observe`. Never call a
private executable path, system Python/Node.js, `pip`, `npm`, inline code, a heredoc, or shell
redirection.

## Bind inputs declaratively

Every input needed by a Builder must be explicit in `run_command.inputs`:

```json
{
  "command": "node scripts/build_deck.mjs --output outputs/product-intro.pptx --image media/hero.png",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "media/hero.png",
      "path": "image-artifact://sha256/<exact-digest>"
    }
  ],
  "reason": "Build the presentation and track its output"
}
```

Supported paths are workspace-relative paths, authorized absolute/system paths, exact attachment
`readPath` values, exact generated `image-artifact://...` paths, and exact revision-bound
`skill://...` URIs. The Host resolves the internal source type.

`mountPath` is an optional private input-root-relative filename and defaults to the source filename. The Host freezes and revalidates the input,
then exposes the run-scoped root in `MYCOPILOT_INPUT_ROOT`. The Builder resolves
`MYCOPILOT_INPUT_ROOT / mountPath`. Never let Python or Node.js open `@attachments`, `skill://`, an
attachment library path, or another private storage path directly.

## Observe every file effect

Every Builder command must declare its expected `.pptx` with exactly one static `--output`
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
prove that the deck is valid.

## Consume render outputs

Render every slide to an explicit contact-sheet image:

```json
{
  "operation": "render",
  "filePath": "outputs/product-intro.pptx",
  "outputPath": "outputs/product-intro-preview.png",
  "reason": "Render every final slide for visual review"
}
```

After a successful render, take the exact path from `outputs[].readPath` and pass it as
`read_image.path`:

```json
{
  "path": "outputs/product-intro-preview.png"
}
```

Treat the returned output as authoritative:

- Select the output whose `role` is `render` and whose `kind` is `image`.
- Use only its `readPath`; never reconstruct a path from the render request, `source`, `argv`,
  `cwd`, `stdout`, or a file search.
- Never rerender merely to discover where the first render was published.
- `pageSelection` records the requested selection (`all` or explicit slide numbers); it is not an
  independent proof of the deck's actual slide count. Establish the final slide count with
  `inspect`, compare it with the request, and then inspect the returned image.
- If the output is absent, not readable, or `read_image` reports an unsupported model capability,
  state that visual verification was unavailable.
- If a preview exceeds the visual-input limit, render bounded slide groups rather than repeating
  the same oversized request. Do not claim inspection of unread images.

## Generate, verify, render, iterate

Use this fixed loop for a final deck:

1. Generate or edit the `.pptx`.
2. Confirm the expected file effect.
3. Inspect slide count and order, titles, text, notes, media, tables, charts, and required content.
4. Run native `validate`.
5. Run one native `render` request covering every final slide with a contact-sheet grid.
6. Read the exact returned render output with `read_image`, then visually inspect every slide for
   overflow, overlap, off-canvas objects, broken media, font substitution, alignment, contrast,
   spacing, and continuity with neighboring slides.
7. If a defect exists, patch the same Builder or issue one corrected semantic operation,
   regenerate, and repeat validation and rendering.

Do not claim visual quality from package validation alone. If rendering returns an
`office.render_backend_*` error, report that visual verification was unavailable; do not launch a
user browser or render every slide in separate retry calls.

## Presentation quality checks

- Keep one clear idea per slide and maintain readable presentation-scale type.
- Use a coherent color, typography, margin, alignment, and spacing system.
- Preserve masters, layouts, notes, media, and unrelated slides during edits.
- Check image cropping, chart labels, connector alignment, and safe page margins.
- Treat a structurally valid deck as necessary but not sufficient for visual success.
- Report only checks that actually ran.
