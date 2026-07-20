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

Call `office_presentation` with one JSON object per operation. `arguments` is an array of literal OfficeCLI tokens, not a shell command: keep each flag and flag value in separate entries. Never add shell quoting, redirects, pipes, an executable name, or a raw batch command.

Before the first presentation operation in a run, check the managed engine:

```json
{ "operation": "status" }
```

Disclose only the schema needed for the next element. For example:

```json
{ "operation": "help", "arguments": ["pptx", "add", "slide", "--json"] }
```

```json
{ "operation": "help", "arguments": ["pptx", "add", "shape", "--json"] }
```

Submit each supported native mutation separately. Prefer the stable canonical element path returned by `add` over guessing a positional shape index.

The managed runtime currently rejects the provider's ambiguous `data` property because it can be
interpreted as either inline content or a local file. Build tables through explicit table, row, and
cell operations instead of passing `--prop data=...`. Diagram-like elements (`diagram`,
`flowchart`, or `mermaid`) must explicitly use `--prop render=native`; browser-backed rendering is
not permitted.

## Script authoring and observation contract

The managed Artifact Runtime exposes these presentation libraries:

| Runtime | Script | Package requirement   | Use                                                                    |
| ------- | ------ | --------------------- | ---------------------------------------------------------------------- |
| Node.js | `.mjs` | `pptxgenjs` `4.0.1`   | Create data-driven `.pptx` decks with reusable JavaScript layout code. |
| Python  | `.py`  | `python-pptx` `1.0.2` | Create or transform `.pptx` content with Python.                       |

Declare only the packages the selected script actually imports. Package names and versions belong
in `runtime.requiredPackages`; never install them from the script.

1. Put substantial program logic in a saved `.py` or `.mjs` file. Do not pass artifact-producing
   code through `python -c`, `node -e`, a heredoc, shell redirection, or another opaque inline form.
2. Make template, data, media, and output paths explicit script arguments. Avoid hard-coded
   machine-specific paths, use deterministic slide and element ordering, and make a rerun
   deterministic where practical.
3. Execute the saved file through `run_command` with the managed Artifact Runtime. Declare every
   runtime package the script requires; dependency resolution is preflight, never implicit package
   installation.
4. Set `observe.kinds` to `["office"]`. List every `.pptx` file that should be created or modified
   in `observe.expectedOutputs`. Paths are resolved relative to the command `cwd` and each expected
   output is observed directly. Do not add its parent merely because the output is outside the
   workspace. Use `observe.additionalRoots` only to scan for other Office changes in an otherwise
   uncovered directory; recursively observing an external directory requires `read=all`.
5. Treat observation as evidence, not authorization or transactionality. It never expands what the
   command may do and does not give an arbitrary script the native tool's staging or rollback
   guarantees.

For example, after creating `scripts/build_deck.mjs` with a file-editing tool, execute that exact
file with the managed Node.js runtime and observe its declared output:

```json
{
  "command": "node scripts/build_deck.mjs --output outputs/quarterly-plan.pptx",
  "cwd": ".",
  "runtime": {
    "provider": "managedArtifact",
    "kind": "node",
    "requiredPackages": [{ "name": "pptxgenjs", "version": "4.0.1" }]
  },
  "observe": {
    "kinds": ["office"],
    "expectedOutputs": ["outputs/quarterly-plan.pptx"]
  },
  "reason": "Generate the requested presentation reproducibly"
}
```

Keep `node` as the logical first command token. The host binds it to the selected managed runtime;
never discover or persist the runtime's private executable path in the script or command.

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
package, and render every changed slide. Do not equate a structurally valid deck with a visually
correct presentation.

## Core recipes

Create a real presentation:

```json
{
  "operation": "create",
  "path": "quarterly-plan.pptx",
  "arguments": ["--json"],
  "reason": "Create the requested PowerPoint presentation"
}
```

Add a slide with a structural title placeholder:

```json
{
  "operation": "add",
  "path": "quarterly-plan.pptx",
  "arguments": [
    "/",
    "--type",
    "slide",
    "--prop",
    "title=Quarterly Plan",
    "--prop",
    "background=F4F7FB",
    "--json"
  ],
  "reason": "Add the title slide"
}
```

Add a positioned text shape to the slide:

```json
{
  "operation": "add",
  "path": "quarterly-plan.pptx",
  "arguments": [
    "/slide[1]",
    "--type",
    "shape",
    "--prop",
    "text=Revenue grew 18%",
    "--prop",
    "x=2cm",
    "--prop",
    "y=4cm",
    "--prop",
    "width=20cm",
    "--prop",
    "height=3cm",
    "--prop",
    "fill=4472C4",
    "--prop",
    "color=FFFFFF",
    "--prop",
    "size=24pt",
    "--json"
  ],
  "reason": "Add the key result shape"
}
```

Read back the slide and its child elements:

```json
{
  "operation": "get",
  "path": "quarterly-plan.pptx",
  "arguments": ["/slide[1]", "--depth", "3", "--json"]
}
```

For the first save-as edit, keep `path` as the frozen source and supply a distinct
`destinationPath`. Apply later mutations to `quarterly-plan.pptx` itself so earlier changes are
preserved:

```json
{
  "operation": "set",
  "path": "template.pptx",
  "destinationPath": "quarterly-plan.pptx",
  "arguments": ["/slide[1]/shape[1]", "--prop", "text=Quarterly Plan", "--json"],
  "reason": "Create the quarterly deck without overwriting the template"
}
```

Render the changed slide to a workspace PNG:

```json
{
  "operation": "view",
  "path": "quarterly-plan.pptx",
  "arguments": ["screenshot", "--page", "1", "--json"],
  "outputPath": "quarterly-plan-slide-1.png",
  "reason": "Render the changed slide for visual inspection"
}
```

Validate the OOXML package:

```json
{ "operation": "validate", "path": "quarterly-plan.pptx", "arguments": ["--json"] }
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
   distinct input and output arguments. Edit in place only when it was explicitly requested.
5. Render every changed slide and compare it with surrounding slides for continuity.

## Quality checks

- Verify slide count and order, title hierarchy, text fit, alignment, contrast, image cropping, chart labels, and speaker notes requested by the user.
- Inspect rendered slides for overlaps, off-canvas objects, missing fonts, broken media, and inconsistent spacing.
- Treat a successful engine exit as necessary but insufficient for a visually acceptable deck.
- Treat a successful script exit as necessary but insufficient; require matching complete artifact
  observation plus structural, package, and visual checks appropriate to the task.
- If a requested feature is not reported by `help` or validation, describe the limitation instead of simulating support.
- Report only checks that actually ran, including any validation or rendering warnings.
