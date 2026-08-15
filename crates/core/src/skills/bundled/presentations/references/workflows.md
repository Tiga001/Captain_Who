# Presentation workflows

## Contents

- [Route the task](#route-the-task)
- [Native semantic contract](#native-semantic-contract)
- [Create and reuse one Builder](#create-and-reuse-one-builder)
- [Edit an existing deck with one Editor](#edit-an-existing-deck-with-one-editor)
- [Clean up managed scripts](#clean-up-managed-scripts)
- [Preflight every Builder revision](#preflight-every-builder-revision)
- [Bind inputs declaratively](#bind-inputs-declaratively)
- [Observe every file effect](#observe-every-file-effect)
- [Consume render outputs](#consume-render-outputs)
- [Generate, verify, render, iterate](#generate-verify-render-iterate)
- [Presentation quality checks](#presentation-quality-checks)

## Route the task

Use `office_presentation` only for `status`, `inspect`, `validate`, and `render`. The
model-facing Office tool is intentionally read/verification-only; do not call or invent create or
mutation operations.

Use the Managed Builder for a new complete visual narrative, repeated layouts, a coordinated
theme, complex diagrams, template population, many slides or media items, or batch generation.

For every edit to an existing `.pptx`, inspect first and then use the fixed Managed Editor from
`templates/editor.mjs`. It emits a typed plan through `@mycopilot/presentation-sdk`; it does not
open or rewrite the package itself. Do not recreate an existing deck with the Builder, and do not
emulate an unsupported edit with low-level OfficeCLI fields. Native inspect/validate/render around
one Builder creation or one Editor transaction is the supported hybrid flow.

## Native semantic contract

Call `office_presentation` with a flat read/verification object:

```json
{
  "operation": "inspect",
  "filePath": "product-intro.pptx",
  "reason": "Inspect the presentation before editing it"
}
```

Keep `operation`, its semantic fields, and `reason` at the root. Never add a `request` wrapper,
provider arguments, an executable, slide DOM paths, or shell flags. `reason` is required,
user-visible audit text only; it never grants permission.

Bind image, template, data, and deck inputs through the selected Builder or Editor command. Inspect
an existing deck before mutation. Establish the audience, purpose, slide count, aspect ratio,
narrative, and visual direction before creating slides.

## Create and reuse one Builder

The bundled `templates/builder.mjs` is a compact `pptxgenjs` starting point with a wide layout,
theme, reusable footer, and optional mounted image. Locate its exact revision-bound URI with
`skills_list_resources`. Choose one dedicated workspace-relative script directory for this run.
If the directory is absent, create it before materialization with a separate idempotent command;
if it already exists as a plain directory, reuse it:

```json
{
  "command": "mkdir -p scripts",
  "cwd": ".",
  "reason": "Prepare a workspace directory for the presentation Builder"
}
```

The name `scripts` is only an example; keep the chosen directory consistent throughout the run.
Then copy the Builder once into that directory:

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

## Edit an existing deck with one Editor

Read [editing-existing.md](editing-existing.md) before the first existing-deck edit in a run. The
fixed sequence is inspect and copy stable targets, render a visual baseline, materialize one
`editor.mjs`, patch only its bounded edit region, syntax-check, execute one transaction, then
inspect, validate, and render/read every final slide.

Create or reuse the dedicated script directory before materializing the Editor, exactly as in the
Builder workflow. Do not wait for `skills_materialize_resource` to fail before creating a missing
parent directory.

The Editor command has exactly one static `--source` and one static `--output`. Bind the source and
every replacement asset through `run_command.inputs`; use its logical `mountPath` in `--source`
and `input(...)`. Prefer a distinct output:

```json
{
  "command": "node scripts/edit_deck.mjs --source source.pptx --output source-edited.pptx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source.pptx",
      "path": "<exact source path or readPath>"
    }
  ],
  "reason": "Apply the reviewed changes to a private copy of the existing presentation"
}
```

## Clean up managed scripts

After the final deck has been published and all required inspection, validation, rendering, and
retries are complete, delete the exact materialized Builder or Editor and every temporary file
created for this task. If this task created the script directory and it is now empty, remove it
with non-recursive `rmdir`. If the directory existed before the task, leave the directory and all
unrelated contents untouched. Never use `rm -rf` for this cleanup. Keep the script only when the
user explicitly asks for it.

Copy stable target strings verbatim from the latest `office_presentation` inspect result, for
example `/slide[3]/shape[@id=42]`. A missing or stale target aborts the transaction. Never
substitute a guessed array index, visible-text-only selector, relationship ID, XML part, or
hand-written object path. The Host owns source freezing, target and plan validation, private
staging, final package validation, and atomic publication. The MJS must import only
`editPresentation`, `input`, and `output` from `@mycopilot/presentation-sdk`; it must not import
filesystem, process-launch, archive, XML, network, or `pptxgenjs` modules and must not invoke
OfficeCLI.

Element `target`, `copyFrom`, and position references must contain inspected identities. A
whole-slide addition, removal, or reordering must use the documented bounded Editor operation; at
most one whole-slide structural operation is allowed and it must be last. Re-inspect before any
following Editor transaction because slide indices and element targets may have changed.

Editor v1 supports save-as only. Use a distinct output and never point `--output` at the mounted
source. Prefer a workspace-root output name unless its parent directory already exists. If the user
requested in-place editing, preserve the original, deliver a distinct validated output, and
disclose that final replacement was not performed. If a typed edit is unsupported, preserve the
structured error and report the limitation; do not unzip/rezip OOXML, rebuild the deck, flatten it
into images, or invent an SDK method.

## Preflight every Builder revision

Immediately after materializing or patching an `.mjs` Builder, syntax-check that exact file in its
own `run_command` call:

```json
{
  "command": "node --check scripts/build_deck.mjs",
  "cwd": ".",
  "reason": "Check the presentation builder syntax before execution"
}
```

Do not join the check to a build with `&&`, a pipe (`|`), or a semicolon (`;`). If the check exits
non-zero, do not build: patch the same file and check it again. Editing the file after a successful
check invalidates that result, so check the new revision before executing it.

If execution fails, inspect the error and `artifactObservation`, then repair the Builder source,
`run_command.inputs`, or provenance/materialization state that caused it. Repeat the syntax check
before the next build even when the source did not change. Never retry the same failing Builder
command unchanged or vary only `reason` or the output filename to evade duplicate-call protection.

Keep the three gates distinct: `syntax-valid != runtime-valid != PPTX-valid`. `node --check` proves
only that Node.js can parse the file. A successful Builder run is still required for runtime/API
correctness, followed by native package validation and rendering for PPTX and visual correctness.

Apply the same syntax discipline to an Editor revision with
`node --check scripts/edit_deck.mjs` in its own call. Do not combine it with execution. For an
existing-deck edit the complete gate sequence is
`syntax-valid != edit-plan-valid != runtime-valid != PPTX-valid != visually-verified`.

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

An Editor uses the same declarative input field, but accesses it only through
`input(logicalMountPath)`. Do not read `MYCOPILOT_INPUT_ROOT` or construct filesystem paths inside
`editor.mjs`; the Host-backed facade resolves the logical handle.

## Observe every file effect

Every Builder build command must declare its expected `.pptx` with exactly one static `--output`
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

Every Editor command also declares exactly one static `.pptx` `--output`, but its effect is
transactional: the Host applies the typed plan to a private candidate, validates it, rechecks
approval, cancellation, and destination preconditions, and only then publishes. A pre-publication
failure must leave the source and destination unchanged. `artifactObservation` remains evidence of
the resulting file effect, not proof of fidelity or visual quality.

## Consume render outputs

First inspect the final deck and record its authoritative slide count `N` and ordered slides. Repeat
this inspection after the last edit because earlier counts become stale:

```json
{
  "operation": "inspect",
  "filePath": "outputs/product-intro.pptx",
  "reason": "Inspect the final slide count and order before visual review"
}
```

A whole-deck contact sheet may be rendered once for narrative flow and layout rhythm, but it is an
overview only. It cannot prove that every slide is present, fully visible, or readable:

```json
{
  "operation": "render",
  "filePath": "outputs/product-intro.pptx",
  "outputPath": "outputs/product-intro-overview.png",
  "reason": "Render an overview contact sheet for narrative review"
}
```

For final visual verification, render each slide independently. Use one call per slide, set
`pageOrSlide` to that one-based slide number, and give every call a unique `outputPath`:

```json
{
  "operation": "render",
  "filePath": "outputs/product-intro.pptx",
  "pageOrSlide": 1,
  "outputPath": "outputs/product-intro-slide-01.png",
  "reason": "Render slide 1 for full-size visual review"
}
```

After each successful render, take the exact path from that call's `outputs[].readPath` and pass it
to `read_image.path`. The path below is illustrative; use the returned value rather than copying
the request path:

```json
{
  "path": "<exact outputs[0].readPath from the slide 1 render>"
}
```

Maintain a numbered verification ledger from slide `1` through slide `N`. Each entry requires a
successful render, a successful `read_image`, and a visual verdict such as `slide 1: PASS` or
`slide 1: text clips at the right edge`. Exactly `N` numbered verdicts are required before claiming
complete visual verification.

Treat render metadata and paths narrowly:

- Select the output whose `role` is `render` and whose `kind` is `image`.
- Use only its `readPath`; never reconstruct a path from the render request, `source`, `argv`,
  `cwd`, `stdout`, or a file search.
- Never rerender merely to discover where the first render was published.
- `total` and `pageSelection` describe source or requested selection metadata; neither proves that
  a returned image contains every slide without clipping.
- `layoutCoverage.evidence = trustedRendererGeometry` proves only that its `requestedPages` fit the
  decoded PNG viewport under the frozen renderer layout. It does not inspect slide pixels, text,
  images, clipping inside a slide, or visual quality, and never replaces the per-slide ledger.
- An output filename such as `complete`, `all`, or `full` is never coverage evidence.
- A contact sheet is never a substitute for the per-slide ledger, even when it looks complete.
- If any output is absent or unreadable, or `read_image` is unavailable, keep that slide's verdict
  missing and state that complete visual verification was unavailable.
- When delegating visual review, pass the numbered slide-to-`readPath` mapping and require one
  verdict for every expected slide. Do not ask a reviewer to infer the slide count from a dense
  contact sheet.

## Generate, verify, render, iterate

Use this fixed loop for a final deck:

1. Generate the `.pptx` with one Builder, or edit an existing deck with one fixed Editor transaction.
2. Confirm the expected file effect.
3. Inspect slide count `N` and order, titles, text, notes, media, tables, charts, and required content.
4. Run native `validate`.
5. Optionally render one whole-deck contact sheet for narrative flow only.
6. Render slides `1..N` independently with `pageOrSlide`, unique output paths, and the exact
   returned `readPath`; read every image and record exactly `N` numbered visual verdicts.
7. Inspect every slide for overflow, overlap, off-canvas objects, broken media, font substitution,
   alignment, contrast, spacing, and continuity with neighboring slides.
8. If a defect exists, patch the same Builder or the same Editor edit region, run the appropriate
   syntax gate, execute one corrected transaction, discard the earlier visual ledger, re-inspect
   `N`, revalidate, and render/read all `N` final slides again. Repeat the overview only when
   narrative continuity may have changed.

Do not claim visual quality from package validation alone. If rendering returns an
`office.render_backend_*` error, report the affected slide as unverified; do not launch a user
browser. Do not claim completion unless the final deck has exactly `N` successful numbered visual
verdicts.

## Presentation quality checks

- Keep one clear idea per slide and maintain readable presentation-scale type.
- Use a coherent color, typography, margin, alignment, and spacing system.
- Preserve masters, layouts, notes, media, and unrelated slides during edits.
- Check image cropping, chart labels, connector alignment, and safe page margins.
- Treat a structurally valid deck as necessary but not sufficient for visual success.
- Report only checks that actually ran.
