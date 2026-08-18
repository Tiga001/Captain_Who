---
name: presentations
description: Create, edit, inspect, render, and validate Microsoft PowerPoint-compatible .pptx presentations. Use for complete slide decks, layouts, themes, text, images, tables, charts, shapes, speaker content, repeated or data-driven generation, and any other presentation task.
---

# Presentations

Route by intent instead of mixing write mechanisms:

- **Read:** use the flat semantic `office_presentation` tool to inspect, render, and validate a deck.
- **Create:** use one saved Managed Builder for every new deck.
- **Edit an existing `.pptx`:** inspect first, then use one saved Managed Editor based on `templates/editor.mjs`. The Editor expresses a typed edit plan through the fixed `@mycopilot/presentation-sdk` facade; the Host applies that plan to a private copy and publishes only after validation. Do not rebuild an existing deck with `pptxgenjs`.

Combine the paths only at their intended boundaries: native inspect/render/validate around one Builder creation or one Editor transaction.

Never expose OfficeCLI arguments, executable paths, runtime versions, or package versions to a model-facing native call. The model-facing Office tool is intentionally read/verification-only and accepts only `status`, `inspect`, `validate`, and `render`; every deck write uses the Builder or Editor. Every native call uses flat top-level semantic fields plus a required `reason`; never wrap it in `request`. The Managed Editor is the one exception for Host-returned stable object targets: copy an exact target such as `/slide[3]/shape[@id=42]` from the `office_presentation` inspect result into the fixed SDK, but never invent one or turn it into OfficeCLI arguments.

## Managed scripts

The Builder and Editor are different fixed entry points:

- `templates/builder.mjs` creates a new deck.
- `templates/editor.mjs` edits one frozen existing deck and defaults to save-as.

Never use the Builder to imitate an edit, and never turn the Editor into a general-purpose Node.js program.

Before materializing a script or calling anything that writes a file, choose the task-owned script
directory and every nested parent directory that the Builder, Editor, or renderer will use. Create
all missing parents first with one separate idempotent `mkdir -p <parents...>` `run_command`. This
includes the materialization destination's parent, the parent of a nested `--output`, and every
render `outputPath` parent. The Host checks these parents before the script or renderer runs, so a
`mkdir` inside Builder code is too late. A workspace-root output needs no directory setup. The
directory names are not fixed, but use the same chosen paths throughout the run. Before the final
response, delete the exact Builder or Editor and any task-created temporary files. If this task
created a directory and it is then empty, remove it with `rmdir`; preserve pre-existing directories
and unrelated files, and never use recursive deletion for this cleanup. Keep a script only when
the user explicitly asks for it.

### Managed Builder

Use `skills_list_resources` to locate `templates/builder.mjs`, ensure the selected script directory
exists, then materialize the Builder once with `skills_materialize_resource` into a new path inside
that directory. Patch and rerun that same builder; do not create a trail of replacement scripts.

Immediately after materializing or modifying any `.mjs` Builder, run `node --check <builder>.mjs` as a separate `run_command` call. Never combine the check and build with `&&`, `|`, or `;`. A non-zero check forbids the build: patch the same Builder and check it again. Any later edit invalidates the successful check.

Execute that materialized file with `run_command` and a direct logical `node <builder>.mjs --output <file.pptx>` command. Omit `runtimeProfile` and `observe`: the host verifies this run's materialization receipt, derives the `presentations` profile from the static Office output, binds the pinned runtime, and observes that output automatically. Never use system Node.js, `pip`, `npm`, inline code, heredocs, or shell redirection.

Bind templates, data, images, attachments, and earlier generated files through `run_command.inputs`:

```json
{
  "mountPath": "media/hero.png",
  "path": "image-artifact://sha256/<exact-digest>"
}
```

Use the exact path returned by the producing tool or supplied by the user. The Host automatically recognizes workspace, absolute/system, `@attachments/...`, `image-artifact://...`, and revision-bound `skill://...` paths. `mountPath` is optional and defaults to the source filename. Builder scripts read only the host-mounted path below `MYCOPILOT_INPUT_ROOT`; never pass or open an `@attachments` or `skill://` URI directly.

Every Builder build command must declare its generated presentation with exactly one static `--output` argument. Inspect the backend-owned `artifactObservation` even after failure, timeout, or cancellation. After a runtime failure, repair the Builder source, input bindings, or provenance/materialization state, then run the syntax check again before rebuilding. Never retry the same failing Builder command unchanged.

Treat the gates independently: `syntax-valid != runtime-valid != PPTX-valid`. The syntax check proves only that Node.js can parse the Builder; execution and final native validation/rendering remain mandatory.

### Managed Editor

Before editing, call `office_presentation` with `operation: "inspect"` and copy exact stable targets from its result while recording the source deck's authoritative slide count and order. Render affected slides when layout or appearance matters. If inspect does not return a stable target for an intended element, stop and report that the edit cannot be applied safely; do not guess from array position, visible text, or a hand-written object path.

Use `skills_list_resources` to locate `templates/editor.mjs`, ensure the selected script directory
exists, materialize the Editor once into a new path inside it, and patch only its bounded
`BEGIN EDIT REGION` / `END EDIT REGION`. Read
[references/editing-existing.md](references/editing-existing.md) before the first existing-deck
edit in a run. Keep using that same Editor file for corrections.

Immediately after materializing or modifying the `.mjs` Editor, run `node --check <editor>.mjs` as a separate `run_command` call. Never combine the check and edit run with `&&`, `|`, or `;`. A non-zero check forbids execution, and any later edit invalidates the successful check.

Run the checked Editor with one direct logical command containing exactly one static `--source` and one static `--output`, for example `node scripts/edit_deck.mjs --source source.pptx --output source-edited.pptx`. Bind the source deck and every replacement asset through `run_command.inputs`; the `--source` value is its logical `mountPath`, not a workspace, attachment, Artifact, or private storage path. Editor v1 supports save-as only, so `--output` must be a distinct workspace-relative `.pptx` destination; absolute paths and parent traversal are invalid. Use a workspace-root filename unless the destination directory already exists. If the user requested in-place editing, preserve the original, produce a distinct validated output, and disclose that final replacement was not performed.

Editor v1 has exactly one external-image route: replace an existing inspected picture with
`deck.replaceImage({ target: '/slide[N]/picture[@id=ID]', source: input('<mountPath>') })`, where
the `input()` value exactly equals one declared `run_command.inputs[].mountPath`. Use it only when
the user's intent is to replace that exact inspected picture; never repurpose an unrelated picture
as a placeholder. Adding a new external image and setting a slide image background are always
unsupported. If the request needs either operation or inspect returns no matching picture, stop
and report the limitation. Outside this exact `replaceImage` call, do not pass an input handle or
guessed `source`, `path`, `src`, `resourcePath`, absolute path, or private placeholder to
`deck.add`, and do not copy Builder image syntax into the Editor.

The Editor may import only `editPresentation`, `input`, and `output` from the fixed `@mycopilot/presentation-sdk` facade. It must not import `pptxgenjs`, filesystem, archive, XML, process-launch, or network modules; invoke OfficeCLI; expose provider arguments; or modify anything outside the edit region. Copy only Host-returned stable targets and use only the documented facade operations. Element targets, `copyFrom`, and position references require inspected identities. Whole-slide structure changes use the Editor's bounded slide operation, at most once and last in the transaction; re-inspect before any following edit transaction. The SDK writes a Host-only typed plan; it never edits the package itself. The Host freezes the source, inputs, script, runtime identity, and destination; validates every target and operation; applies the plan to a private candidate; validates it; and publishes atomically. Model code never receives a real mount, staging, OfficeCLI, or executable path.

Default to fidelity-preserving targeted changes. Do not unzip or rewrite OOXML, rebuild untouched slides, flatten editable content into pictures, discard masters/layouts/notes, or silently substitute an unsupported operation. If the facade or Host rejects a feature, preserve the structured error and tell the user what could not be edited.

## Completion gate

1. Inspect an existing deck before editing it. For edits, copy stable Host-returned targets and use a distinct save-as output; Editor v1 does not replace the source in place.
2. Establish the audience, slide count, narrative, aspect ratio, and visual direction before building.
3. Confirm the expected file effect in the native result or `artifactObservation`.
4. Inspect the final deck, record its authoritative slide count `N` and slide order, then validate the package.
5. A whole-deck contact sheet is optional and is overview-only. Never use it to prove slide coverage or per-slide visual quality.
6. For every slide `1..N`, make a separate `render` call with that `pageOrSlide` and a unique `outputPath`. Pass the exact returned `outputs[].readPath` to `read_image.path` and record one numbered visual verdict for that slide. Any later deck edit invalidates the ledger; re-inspect, revalidate, and rebuild all `N` verdicts from the final deck.
7. `outputs[].layoutCoverage` proves only that the frozen requested slide set fits inside the PNG viewport under the trusted renderer's fixed layout geometry. It does not prove slide content, visual quality, or successful per-slide inspection. Do not infer visual coverage from it, `total`, `pageSelection`, an output filename, or a contact-sheet image. Without exactly `N` successful numbered verdicts, do not claim complete visual verification or completion.
8. Report only the file effects and checks that actually succeeded. Preserve structured errors and disclose unavailable visual verification.
9. After all required retries and verification are complete, clean up the task-owned Builder or
   Editor and temporary files. Remove the script directory only if this task created it and it is
   empty; never delete a pre-existing directory or unrelated files.

Read [references/workflows.md](references/workflows.md) for routing, creation, input binding, verification, and presentation-specific quality checks. Read [references/editing-existing.md](references/editing-existing.md) for the fixed MJS editing contract, supported operations, stable-target rules, recovery, and fidelity checks.
