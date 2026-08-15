# Editing an existing presentation

Use this workflow for any change to an existing `.pptx`. It preserves the original deck's
masters, layouts, notes, theme, media, and untouched slides by applying a bounded edit plan to a
private copy. It is not a second deck-generation workflow.

## Contents

- [Fixed workflow](#fixed-workflow)
- [Materialize and run one Editor](#materialize-and-run-one-editor)
- [Copy stable targets from native inspection](#copy-stable-targets-from-native-inspection)
- [Supported facade operations](#supported-facade-operations)
- [Host-owned safety and transaction boundary](#host-owned-safety-and-transaction-boundary)
- [Forbidden recovery paths](#forbidden-recovery-paths)
- [Fidelity and completion checks](#fidelity-and-completion-checks)

## Fixed workflow

1. Call `office_presentation` with `operation: "inspect"` before writing. Record the authoritative
   slide count and order, then copy the exact stable target string returned for every intended
   element, such as `/slide[3]/shape[@id=42]`.
2. Render and read every slide whose layout or appearance may change. These images are the visual
   baseline; a contact sheet may help with overview but is not per-slide evidence.
3. Choose one dedicated workspace-relative script directory. Reuse it if it already exists as a
   plain directory; otherwise create it first with a separate idempotent
   `mkdir -p <script-directory>` `run_command`.
4. Locate the exact revision-bound `templates/editor.mjs` URI with `skills_list_resources` and
   materialize it once to a new path such as `scripts/edit_deck.mjs` inside that existing directory.
5. Patch only the template's `BEGIN EDIT REGION` / `END EDIT REGION`. Keep the fixed import,
   argument parsing, `editPresentation`, `input`, `output`, and `mode: 'saveAs'` code unchanged.
6. Run `node --check scripts/edit_deck.mjs` in a separate `run_command`. If it fails, patch the
   same file and check it again. Any later patch invalidates the successful check.
7. Run the Editor once with exactly one static `--source` and one static `--output`. Bind the source
   and every replacement asset through `run_command.inputs`.
   If `run_command` returns `status: "running"`, the edit is still inside the Host-owned Office
   transaction: follow its `continueWith` receipt and call `command_session` with `action: "wait"`
   until the authoritative terminal result succeeds. A running receipt, Node exit, output file
   name, or preliminary observation is never proof that the edited deck was published.
8. Only after terminal success and an `artifactObservation` confirming the declared destination,
   inspect the resulting deck again, validate it, render every final slide separately, read every
   returned image, and rebuild the numbered visual verdict ledger. Compare affected slides with
   the baseline and confirm unrelated slides remain intact.
9. After all corrections and verification are complete, delete the exact Editor and task-created
   temporary files. If this task created the script directory and it is empty, remove it with
   `rmdir`; otherwise preserve the directory and every unrelated file. Never use recursive deletion.

Do not skip directly from syntax validation to delivery. The independent gates are:

`syntax-valid != edit-plan-valid != runtime-valid != PPTX-valid != visually-verified`

## Materialize and run one Editor

Materialize the exact revision returned by the active Skill. The URI below is illustrative only:

First create or reuse a dedicated directory. This command is intentionally idempotent:

```json
{
  "command": "mkdir -p scripts",
  "cwd": ".",
  "reason": "Prepare a workspace directory for the presentation Editor"
}
```

The directory name is not fixed. Use the same chosen path in the materialization, patch, syntax
check, and edit command. Do not wait for `skills_materialize_resource` to fail before creating a
missing parent directory.

```json
{
  "sourceUri": "skill://package/<exact-revision>/templates/editor.mjs",
  "destination": "scripts/edit_deck.mjs",
  "reason": "Create the fixed presentation editor for the requested existing-deck changes"
}
```

Use the same script for every correction. Never rematerialize over a modified file or create a
sequence of replacement editors.

After patching, run the syntax gate by itself:

```json
{
  "command": "node --check scripts/edit_deck.mjs",
  "cwd": ".",
  "reason": "Check the presentation editor syntax before applying its edit plan"
}
```

Then execute the checked revision. `source.pptx` is a logical mounted name, not the original path:

```json
{
  "command": "node scripts/edit_deck.mjs --source source.pptx --output source-edited.pptx",
  "cwd": ".",
  "inputs": [
    {
      "mountPath": "source.pptx",
      "path": "<exact source path or readPath>"
    },
    {
      "mountPath": "media/replacement.png",
      "path": "<exact image readPath or image-artifact URI>"
    }
  ],
  "reason": "Apply the reviewed edits to a private copy of the presentation and save a validated result"
}
```

The `input()` values in the MJS must equal declared `inputs[].mountPath` values. The `output()`
value comes from the single static `--output`; it is a logical destination controlled by the
Host. Do not read `MYCOPILOT_INPUT_ROOT`, construct a real path, or discover a private directory in
the Editor.

Editor v1 supports only `mode: 'saveAs'` to a distinct workspace-relative `.pptx`; absolute paths
and parent traversal are invalid. If the user requests in-place replacement, preserve the
original, create and verify a distinct output, and disclose that final replacement was not
performed. Prefer a workspace-root output name unless its parent directory already exists. Never
simulate in-place editing by deleting, renaming, copying, or overwriting files from MJS.

## Copy stable targets from native inspection

An edit is safe only when its target came from the latest frozen-source `office_presentation`
inspect result. Copy the returned stable target verbatim:

```js
deck.replaceText({
  target: '/slide[3]/shape[@id=42]',
  find: 'Old title',
  replace: 'A precise replacement title'
})
```

The target syntax is a Host-issued object identity, not permission to construct OfficeCLI
arguments. Do not invent a path, weaken a failing target, fall back to a guessed shape index, or
select by visible text alone. A missing id, a changed source deck, or a target/type mismatch aborts
the whole transaction; re-inspect the frozen source and copy the new stable target instead.

Keep element kinds exactly as returned. For example, a picture target is
`/slide[2]/picture[@id=17]`, a chart target is `/slide[4]/chart[@id=11]`, and a table-cell target may
be `/slide[3]/table[@id=9]/row[2]/cell[3]`. Do not rewrite those targets as generic shapes.

Slide numbers are one-based and identify the inspected source order. Editor v1 does not add,
remove, move, or swap whole slides. Use native `addSlide`, `removeSlide`, or `moveSlide` first;
because that changes slide ordering, discard every earlier element target and re-inspect before
opening an Editor transaction.

Never use XPath, relationship IDs, XML part names, ZIP entry names, OfficeCLI argv, `raw-set`, or a
target invented from provider documentation. Use only the exact target strings returned by the
Host for this source revision.

## Supported facade operations

The fixed `@mycopilot/presentation-sdk` facade exposes edit intent, not file or process access.
Use only the surface demonstrated by `templates/editor.mjs`. The following shows the facade shape;
in a materialized template, keep its fixed `requiredValue('--source')` and
`requiredValue('--output')` bindings unchanged:

```js
import { editPresentation, input, output } from '@mycopilot/presentation-sdk'

await editPresentation({
  source: input('source.pptx'),
  destination: output('source-edited.pptx'),
  mode: 'saveAs',
  edit(deck) {
    deck.replaceText({
      target: '/slide[3]/shape[@id=42]',
      find: 'Old title',
      replace: 'New title'
    })
  }
})
```

- `editPresentation({ source: input(...), destination: output(...), mode: 'saveAs', edit })`
  declares one frozen source, one logical destination, and one transaction.
- `deck.set({ target, properties })` updates Host-validated properties on one exact target.
- `deck.replaceText({ target, find, replace })` replaces expected text inside one exact target.
- `deck.add({ parent, elementType, copyFrom?, position?, properties? })` adds one typed element;
  `parent` may be `/slide[N]`, while `copyFrom` and position references require inspected ids.
- `deck.remove({ target, properties? })` removes one exact element target, never a whole slide.
- `deck.move({ target, newParent?, position?, properties? })` moves one exact element target;
  `newParent` may be `/slide[N]`. `position`
  is a tagged object: `{ type: 'index', index }` or
  `{ type: 'after' | 'before', target }`, for example
  `{ type: 'after', target: '/slide[6]/shape[@id=12]' }`; non-index position targets require an
  inspected id. Do not pass a bare number.
- `deck.swap({ firstTarget, secondTarget })` swaps two exact element targets with inspected ids,
  never two whole slides.
- `deck.replaceImage({ target, source: input(...) })` replaces picture content with one declared
  mounted input while preserving the existing element unless explicit properties say otherwise.
- `deck.updateTableCell({ target, text })` updates one Host-returned table-cell target.
- `deck.updateChart({ target, properties: { categories, series } })` updates a chart through the
  Host's typed validator; `categories` and `series` must both be arrays.

The SDK serializes this intent into a Host-only plan; it does not open, unzip, or modify a `.pptx`.
Do not invent methods or property names when the documented surface cannot express a request.
Preserve the Host's structured unsupported error and explain the limitation.

## Host-owned safety and transaction boundary

The MJS does not edit the file itself. At preparation and approval the Host freezes the normalized
script, materialization identity, SDK/runtime identity, conversation/run identity, source and
asset references, hashes, sizes, mount paths, and logical destination. It then:

1. validates every stable target's bounded grammar and element type, then lets the pinned Office
   engine resolve it against the frozen source; a missing, stale, or type-mismatched target fails
   the whole atomic transaction;
2. validates the typed plan and rejects raw provider arguments or undeclared inputs;
3. applies the plan to a private candidate while the source and mounted assets remain read-only;
4. requires a bounded, structurally valid OOXML ZIP with a presentation main part and a successful
   pinned Office validation result;
5. publishes atomically only after cancellation and destination preconditions are rechecked.

Model code never receives OfficeCLI, executable, real mount, staging, attachment-library, or
private output paths. Approval covers one complete edit transaction. A failure before publication
must leave the source and destination unchanged.

## Forbidden recovery paths

Never recover from an unsupported or failed edit by:

- importing `pptxgenjs`, `fs`, `child_process`, network, archive, or XML modules;
- spawning OfficeCLI or another executable;
- unzipping the `.pptx`, patching XML, changing relationships, or rezipping it;
- rebuilding the existing deck from scratch with the Builder;
- flattening editable slides, tables, charts, or text into screenshots;
- weakening or inventing targets until they happen to match;
- installing a package or using system Node.js/Python;
- retrying an unchanged command after it may have produced a file effect.

Password-protected or rights-managed files, macros, OLE/ActiveX objects, SmartArt internals,
animations, complex master/theme surgery, unsupported chart families, and edits without a stable
Host-returned target may be unsupported. Keep the original intact, report the exact rejected
capability, and offer the nearest safe alternative rather than claiming success.

## Fidelity and completion checks

After the edit, compare source and result intentionally:

- Verify that an Editor transaction preserves slide count and order. If a separate native slide
  operation intentionally changed them, compare against the post-native, re-inspected baseline.
- Confirm changed text is neither clipped nor unexpectedly restyled.
- Confirm replacement images retain the requested crop, aspect ratio, transparency, and geometry.
- Confirm table cell borders, fills, fonts, merged cells, and dimensions remain intact outside the
  requested cells.
- Confirm chart type, theme, axes, legend, labels, number formats, and position remain intact unless
  the request changed them.
- Check masters, layouts, notes, hyperlinks, media, and untouched slides for preservation.
- Validate the package, then render and read every final slide separately. Exactly one numbered
  verdict per inspected final slide is required; a contact sheet is never enough.

If any final edit changes the deck, discard the earlier validation and visual ledger, then repeat
inspection, validation, and all per-slide render/read verdicts from the new final file.
