---
name: presentations
description: Create, edit, inspect, render, and validate Microsoft PowerPoint-compatible .pptx presentations. Use for complete slide decks, layouts, themes, text, images, tables, charts, shapes, speaker content, repeated or data-driven generation, and any other presentation task.
---

# Presentations

Use one of two supported paths:

- Prefer the flat semantic `office_presentation` tool for bounded, high-frequency work such as creating a deck, adding, moving or removing slides, adding text, images, tables, charts, shapes or footers, inspecting, rendering, and validating.
- Use one saved Managed Builder for complete visual narratives, repeated layouts, coordinated themes, complex diagrams, template population, bulk generation, or any capability the native tool reports as unsupported.
- Combine the paths when useful: build with a script, then inspect, render, and validate with the native tool.

Never expose OfficeCLI arguments, slide DOM paths, executable paths, runtime versions, or package versions to the model-facing call. Every native call uses flat top-level semantic fields plus a required `reason`; never wrap it in `request`. Keep `reason` to one non-empty user-facing sentence of at most 240 characters. When the native tool returns `capabilityNotSupported` with `recovery=useManagedScript`, switch once to the Builder path instead of guessing low-level fields or repeating the failed call.

## Managed Builder

Use `skills_list_resources` to locate `templates/builder.mjs`, then materialize it once with `skills_materialize_resource` into a new workspace path. Patch and rerun that same builder; do not create a trail of replacement scripts.

Immediately after materializing or modifying any `.mjs` Builder, run `node --check <builder>.mjs` as a separate `run_command` call. Never combine the check and build with `&&`, `|`, or `;`. A non-zero check forbids the build: patch the same Builder and check it again. Any later edit invalidates the successful check.

Execute that materialized file with `run_command` and a direct logical `node <builder>.mjs --output <file.pptx>` command. Omit `runtimeProfile` and `observe`: the host verifies this run's materialization receipt, derives the `presentations` profile from the static Office output, binds the pinned runtime, and observes that output automatically. Never use system Node.js, `pip`, `npm`, inline code, heredocs, or shell redirection.

Bind templates, data, images, attachments, and earlier generated files through `run_command.inputs`:

```json
{
  "mountPath": "media/hero.png",
  "path": "image-artifact://sha256/<exact-digest>"
}
```

Use the exact path returned by the producing tool or supplied by the user. The Host automatically recognizes workspace, absolute/system, `@attachments/...`, `image-artifact://...`, and revision-bound `skill://...` paths. `mountPath` is optional and defaults to the source filename. Scripts read only the host-mounted path below `MYCOPILOT_INPUT_ROOT`; never pass or open an `@attachments` or `skill://` URI directly.

Every Builder build command must declare its generated presentation with exactly one static `--output` argument. Inspect the backend-owned `artifactObservation` even after failure, timeout, or cancellation. After a runtime failure, repair the Builder source, input bindings, or provenance/materialization state, then run the syntax check again before rebuilding. Never retry the same failing Builder command unchanged.

Treat the gates independently: `syntax-valid != runtime-valid != PPTX-valid`. The syntax check proves only that Node.js can parse the Builder; execution and final native validation/rendering remain mandatory.

## Completion gate

1. Inspect an existing deck before editing it and prefer a distinct output unless the user requested in-place editing.
2. Establish the audience, slide count, narrative, aspect ratio, and visual direction before building.
3. Confirm the expected file effect in the native result or `artifactObservation`.
4. Inspect the final deck, record its authoritative slide count `N` and slide order, then validate the package.
5. A whole-deck contact sheet is optional and is overview-only. Never use it to prove slide coverage or per-slide visual quality.
6. For every slide `1..N`, make a separate `render` call with that `pageOrSlide` and a unique `outputPath`. Pass the exact returned `outputs[].readPath` to `read_image.path` and record one numbered visual verdict for that slide. Any later deck edit invalidates the ledger; re-inspect, revalidate, and rebuild all `N` verdicts from the final deck.
7. `outputs[].layoutCoverage` proves only that the frozen requested slide set fits inside the PNG viewport under the trusted renderer's fixed layout geometry. It does not prove slide content, visual quality, or successful per-slide inspection. Do not infer visual coverage from it, `total`, `pageSelection`, an output filename, or a contact-sheet image. Without exactly `N` successful numbered verdicts, do not claim complete visual verification or completion.
8. Report only the file effects and checks that actually succeeded. Preserve structured errors and disclose unavailable visual verification.

Read [references/workflows.md](references/workflows.md) for exact semantic and Builder examples, input binding, verification, and presentation-specific quality checks.
