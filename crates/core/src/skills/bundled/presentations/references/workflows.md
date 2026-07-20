# Presentation workflows

## Native tool contract

Call `office_presentation` with one JSON object per operation. `arguments` is an array of literal OfficeCLI tokens, not a shell command: keep each flag and flag value in separate entries. Never add shell quoting, redirects, pipes, an executable name, or a raw batch command.

Before the first presentation operation in a run, check the managed engine:

```json
{"operation":"status"}
```

Disclose only the schema needed for the next element. For example:

```json
{"operation":"help","arguments":["pptx","add","slide","--json"]}
```

```json
{"operation":"help","arguments":["pptx","add","shape","--json"]}
```

Submit each supported native mutation separately. Prefer the stable canonical element path returned by `add` over guessing a positional shape index.

The managed runtime currently rejects the provider's ambiguous `data` property because it can be
interpreted as either inline content or a local file. Build tables through explicit table, row, and
cell operations instead of passing `--prop data=...`. Diagram-like elements (`diagram`,
`flowchart`, or `mermaid`) must explicitly use `--prop render=native`; browser-backed rendering is
not permitted.

## Core recipes

Create a real presentation:

```json
{"operation":"create","path":"quarterly-plan.pptx","arguments":["--json"],"reason":"Create the requested PowerPoint presentation"}
```

Add a slide with a structural title placeholder:

```json
{"operation":"add","path":"quarterly-plan.pptx","arguments":["/","--type","slide","--prop","title=Quarterly Plan","--prop","background=F4F7FB","--json"],"reason":"Add the title slide"}
```

Add a positioned text shape to the slide:

```json
{"operation":"add","path":"quarterly-plan.pptx","arguments":["/slide[1]","--type","shape","--prop","text=Revenue grew 18%","--prop","x=2cm","--prop","y=4cm","--prop","width=20cm","--prop","height=3cm","--prop","fill=4472C4","--prop","color=FFFFFF","--prop","size=24pt","--json"],"reason":"Add the key result shape"}
```

Read back the slide and its child elements:

```json
{"operation":"get","path":"quarterly-plan.pptx","arguments":["/slide[1]","--depth","3","--json"]}
```

For the first save-as edit, keep `path` as the frozen source and supply a distinct
`destinationPath`. Apply later mutations to `quarterly-plan.pptx` itself so earlier changes are
preserved:

```json
{"operation":"set","path":"template.pptx","destinationPath":"quarterly-plan.pptx","arguments":["/slide[1]/shape[1]","--prop","text=Quarterly Plan","--json"],"reason":"Create the quarterly deck without overwriting the template"}
```

Render the changed slide to a workspace PNG:

```json
{"operation":"view","path":"quarterly-plan.pptx","arguments":["screenshot","--page","1","--json"],"outputPath":"quarterly-plan-slide-1.png","reason":"Render the changed slide for visual inspection"}
```

Validate the OOXML package:

```json
{"operation":"validate","path":"quarterly-plan.pptx","arguments":["--json"]}
```

## Create

1. Establish the audience, purpose, slide count, aspect ratio, narrative, and visual direction.
2. Create a real `.pptx` through `office_presentation`; use slide layouts and theme elements consistently.
3. Keep one clear idea per slide. Prefer concise text, readable charts, and purposeful visuals over dense paragraphs.
4. Align and distribute elements precisely. Preserve safe margins and readable contrast at presentation scale.
5. Render all slides and inspect the complete deck before delivery.

## Edit

1. Inspect slide order, layouts, masters, theme, notes, and media before changing the deck.
2. Preserve the established visual system and unrelated content unless the user requests a redesign.
3. Prefer targeted slide or object operations over rebuilding the deck.
4. Use `destinationPath` to save to a new deck unless in-place editing was explicitly requested.
5. Render every changed slide and compare it with surrounding slides for continuity.

## Quality checks

- Verify slide count and order, title hierarchy, text fit, alignment, contrast, image cropping, chart labels, and speaker notes requested by the user.
- Inspect rendered slides for overlaps, off-canvas objects, missing fonts, broken media, and inconsistent spacing.
- Treat a successful engine exit as necessary but insufficient for a visually acceptable deck.
- If a requested feature is not reported by `help` or validation, describe the limitation instead of simulating support.
- Report only checks that actually ran, including any validation or rendering warnings.
