# Presentation workflows

## Choosing an execution path

Use the native path for deck inspection, isolated slide or element changes, and other bounded
operations that `office_presentation` represents directly. It provides a frozen structured action,
conflict checks, staging, and atomic publication for writes.

Use a reproducible script when the task is substantially clearer as code: data-driven decks,
repeated layouts, coordinated theme construction, large batches of slides or media, template
population, or a rerunnable presentation builder. Keep the generator in a reviewable `.py` or
`.mjs` file. A script is not a shortcut around file-edit or command authorization; create or update
it with `apply_patch` or `write_file`, then execute that exact saved file with `run_command` and the
managed Artifact Runtime.

The two paths compose. A script may produce the deck, after which native structural inspection,
rendering, and validation provide package and visual verification. Prefer a new output deck for
scripted transformations unless the user explicitly requested an in-place edit.

## Native tool contract

Call `office_presentation` once per operation with exactly this root envelope: `{ "request": { "operation": "...", ... }, "reason": "..." }`. Put every operation and operation-specific field inside `request`. Keep `reason` as the only root field. Every call, including `status`, `help`, `get`, `query`, `validate`, `view`, and every mutation, must include it. Write `reason` as one non-empty, single-line plain-text sentence in the user's language, no longer than 240 characters, describing the user-visible purpose of this specific call. Do not include line breaks, control characters, or bidirectional text controls. Do not use it to assert success or authorization: it is untrusted display and audit metadata only and never grants permission, approval, or access.

The native request is typed and operation-specific. Put the deck in `request.filePath`; use `request.target` for an exact element path, `request.parent` plus `request.element` for insertion, `request.properties` for element content and layout, and `request.pages` plus `request.outputPath` for rendering. Never send provider command tokens, document-format tokens, output flags, or a raw batch command. The host validates the typed request, generates deterministic provider argv, freezes it for approval, and regenerates it before execution.

Before the first presentation operation in a run, check the managed engine:

```json
{ "request": { "operation": "status" }, "reason": "Check whether presentation tools are available" }
```

Disclose only the schema needed for the next element. For example:

```json
{
  "request": {
    "operation": "help",
    "verb": "add",
    "element": "slide"
  },
  "reason": "Check how to add the requested presentation slide"
}
```

```json
{
  "request": {
    "operation": "help",
    "verb": "add",
    "element": "shape"
  },
  "reason": "Check how to add the requested presentation shape"
}
```

Submit each supported native mutation separately. Prefer the stable canonical element path returned by `add` over guessing a positional shape index.

The managed runtime currently rejects the provider's ambiguous `data` property because it can be
interpreted as either inline content or a local file. Build tables through explicit table, row, and
cell operations instead of using a `data` property. For diagram-like elements (`diagram`,
`flowchart`, or `mermaid`), the host forces the safe native renderer; browser-backed rendering is
not permitted and the model does not select it.

## Script authoring and observation contract

Select the `presentations` runtime profile. The logical saved-script command selects its runtime
family; the model does not select a provider, runtime kind, dependency set, or version:

| Command  | Script | Profile library               | Use                                                                    |
| -------- | ------ | ----------------------------- | ---------------------------------------------------------------------- |
| `node`   | `.mjs` | `pptxgenjs`                   | Create data-driven `.pptx` decks with reusable JavaScript layout code. |
| `python` | `.py`  | `python-pptx` (`pptx` import) | Create or transform `.pptx` content with Python.                       |

Set the top-level `runtimeProfile` field to `presentations`. Do not send a `runtime` object and do
not copy a provider, kind, package name, or package version into the tool call. The host maps
`node` or `python` to the matching profile entry, resolves the pinned dependencies, verifies the
managed runtime, and freezes the exact resolution and integrity identity before execution.

1. Put substantial program logic in a saved `.py` or `.mjs` file. Do not pass artifact-producing
   code through `python -c`, `node -e`, a heredoc, shell redirection, or another opaque inline form.
2. Make template, data, media, and output paths explicit script parameters. Avoid hard-coded
   machine-specific paths, use deterministic slide and element ordering, and make a rerun
   deterministic where practical.
3. Execute the saved file through `run_command` with `runtimeProfile="presentations"`. Dependency
   and runtime resolution is host-owned preflight, never implicit package installation.
4. Set `observe.kinds` to `["office"]`. List every `.pptx` file that should be created or modified
   in `observe.expectedOutputs`. Paths are resolved relative to the command `cwd` and each expected
   output is observed directly. Do not add its parent merely because the output is outside the
   workspace. Use `observe.additionalRoots` only to scan for other Office changes in an otherwise
   uncovered directory; recursively observing an external directory requires `read=all`.
5. Treat observation as evidence, not authorization or transactionality. It never expands what the
   command may do and does not give an arbitrary script the native tool's staging or rollback
   guarantees.

For example, after creating `scripts/build_deck.mjs` with a file-editing tool, run the Node.js
entrypoint and observe its declared output:

```json
{
  "command": "node scripts/build_deck.mjs --output outputs/quarterly-plan.pptx",
  "cwd": ".",
  "runtimeProfile": "presentations",
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/quarterly-plan.pptx"]
  },
  "reason": "Generate the requested presentation reproducibly"
}
```

The equivalent Python route keeps the same profile and changes only the reviewed script and its
logical entrypoint:

```json
{
  "command": "python scripts/build_deck.py --output outputs/quarterly-plan.pptx",
  "cwd": ".",
  "runtimeProfile": "presentations",
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/quarterly-plan.pptx"]
  },
  "reason": "Generate the requested presentation reproducibly"
}
```

Keep `node` or `python` as the logical first command token. The host binds it to the profile's
managed executable; never discover or persist a private executable path. If profile preflight
fails, preserve that error. Do not remove `runtimeProfile`, use a system executable, install a
package, or guess another version. Use the native Office path only when it supports the requested
work, or rewrite and review a script for the other profile entrypoint before retrying with the same
profile. Never run a `.mjs` file as Python or a `.py` file as Node.js.

After every observed command, inspect `artifactObservation` even if the command failed, timed out,
or was cancelled:

- `status=complete` means the scan finished within its reported coverage policy, not that every
  filesystem entry was inspected. Always inspect excluded directories, warnings, and
  `changesTruncated`; `partial` or `failed` must be disclosed and cannot establish that no other
  Office file changed.
- Match every requested output in `artifactObservation.expectedOutputs`. Only `created`, `modified`,
  `replaced`, or `renamed` establish a file effect. `unchanged`, `missing`, `unobserved`, or
  `invalid` do not satisfy a requested edit.
- Inspect `changes` for unexpected deck creation, replacement, deletion, or rename, and retain
  every warning. A non-zero exit can still leave file effects; a zero exit does not prove that the
  expected presentation exists or is valid.
- Do not blindly retry after any observed side effect. First inspect the resulting deck and decide
  whether to continue from it, overwrite it deliberately, or report the partial outcome.

Observation is followed by presentation verification. Read the relevant structure, validate the
package, and render every changed slide. Put all changed slide ranges into one screenshot request
with a contact-sheet grid; never issue one browser-backed call per slide. Render a single slide
again only after the combined preview exposes a concrete defect. If the managed renderer returns
an `office.render_backend_*` error, preserve it and report the visual check as unavailable instead
of falling back to a user browser. Do not equate a structurally valid deck with a visually correct
presentation.

## Core recipes

Create a real presentation:

```json
{
  "request": {
    "operation": "create",
    "filePath": "quarterly-plan.pptx"
  },
  "reason": "Create the requested PowerPoint presentation"
}
```

Add a slide with a structural title placeholder:

```json
{
  "request": {
    "operation": "add",
    "filePath": "quarterly-plan.pptx",
    "parent": "/",
    "element": "slide",
    "properties": {
      "title": "Quarterly Plan",
      "background": "F4F7FB"
    }
  },
  "reason": "Add the title slide"
}
```

Add a positioned text shape to the slide:

```json
{
  "request": {
    "operation": "add",
    "filePath": "quarterly-plan.pptx",
    "parent": "/slide[1]",
    "element": "shape",
    "properties": {
      "text": "Revenue grew 18%",
      "x": "2cm",
      "y": "4cm",
      "width": "20cm",
      "height": "3cm",
      "fill": "4472C4",
      "color": "FFFFFF",
      "size": "24pt"
    }
  },
  "reason": "Add the key result shape"
}
```

Read back the slide and its child elements:

```json
{
  "request": {
    "operation": "get",
    "filePath": "quarterly-plan.pptx",
    "target": "/slide[1]",
    "depth": 3
  },
  "reason": "Verify the slide structure and content"
}
```

For the first save-as edit, keep `filePath` as the frozen source and supply a distinct
`destinationPath`. Apply later mutations to `quarterly-plan.pptx` itself so earlier changes are
preserved:

```json
{
  "request": {
    "operation": "set",
    "filePath": "template.pptx",
    "destinationPath": "quarterly-plan.pptx",
    "target": "/slide[1]/shape[1]",
    "properties": { "text": "Quarterly Plan" }
  },
  "reason": "Create the quarterly deck without overwriting the template"
}
```

Render all changed slides to one workspace contact sheet:

```json
{
  "request": {
    "operation": "view",
    "filePath": "quarterly-plan.pptx",
    "mode": "screenshot",
    "pages": [{ "start": 1, "end": 6 }],
    "grid": { "mode": "auto" },
    "outputPath": "quarterly-plan-slides.png"
  },
  "reason": "Render all changed slides for visual inspection"
}
```

Validate the OOXML package:

```json
{
  "request": {
    "operation": "validate",
    "filePath": "quarterly-plan.pptx"
  },
  "reason": "Validate the finished PowerPoint presentation"
}
```

## Create

1. Establish the audience, purpose, slide count, aspect ratio, narrative, and visual direction.
2. Choose native operations for bounded authoring or a saved generator for coordinated,
   repetitive authoring. Always create a real `.pptx` and use slide layouts and theme elements
   consistently.
3. Keep one clear idea per slide. Prefer concise text, readable charts, and purposeful visuals over dense paragraphs.
4. Align and distribute elements precisely. Preserve safe margins and readable contrast at presentation scale.
5. Render all slides and inspect the complete deck before delivery.

## Edit

1. Inspect slide order, layouts, masters, theme, notes, and media before changing the deck.
2. Preserve the established visual system and unrelated content unless the user requests a redesign.
3. Prefer targeted native slide or object operations for local edits. Use a saved script only when
   its coordinated transformation is materially clearer or more reproducible than many isolated calls.
4. With the native path, use `destinationPath` to save to a new deck. With the script path, pass
   distinct input and output parameters. Edit in place only when it was explicitly requested.
5. Render every changed slide and compare it with surrounding slides for continuity.

## Quality checks

- Verify slide count and order, title hierarchy, text fit, alignment, contrast, image cropping, chart labels, and speaker notes requested by the user.
- Inspect rendered slides for overlaps, off-canvas objects, missing fonts, broken media, and inconsistent spacing.
- Treat a successful engine exit as necessary but insufficient for a visually acceptable deck.
- Treat a successful script exit as necessary but insufficient; require matching complete artifact
  observation plus structural, package, and visual checks appropriate to the task.
- If a requested feature is not reported by `help` or validation, describe the limitation instead of simulating support.
- Report only checks that actually ran, including any validation or rendering warnings.
