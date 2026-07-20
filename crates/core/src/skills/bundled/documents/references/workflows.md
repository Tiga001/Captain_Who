# Word document workflows

## Native tool contract

Call `office_document` with one JSON object per operation. `arguments` is an array of literal OfficeCLI tokens, not a shell command: keep every flag and every flag value in separate array entries. Never add shell quoting, redirects, pipes, or an executable name.

Before the first document operation in a run, check the managed engine:

```json
{"operation":"status"}
```

When an element, property, or DOM path is uncertain, disclose only the relevant provider schema. For example:

```json
{"operation":"help","arguments":["docx","add","paragraph","--json"]}
```

Do not use a raw batch command. Submit each supported native operation separately and inspect its result before depending on it.

The managed runtime currently rejects the provider's ambiguous `data` property because it can be
interpreted as either inline content or a local file. Build tables through explicit table, row, and
cell operations instead of passing `--prop data=...`.

## Core recipes

Create a real Word document:

```json
{"operation":"create","path":"report.docx","arguments":["--locale","zh-CN","--json"],"reason":"Create the requested Word document"}
```

Add a structurally styled heading, then a normal paragraph:

```json
{"operation":"add","path":"report.docx","arguments":["/body","--type","paragraph","--prop","text=Quarterly Report","--prop","style=Heading1","--json"],"reason":"Add the document heading"}
```

```json
{"operation":"add","path":"report.docx","arguments":["/body","--type","paragraph","--prop","text=Revenue and operating results are summarized below.","--prop","style=Normal","--json"],"reason":"Add the introductory paragraph"}
```

Read back the body structure and content:

```json
{"operation":"get","path":"report.docx","arguments":["/body","--depth","2","--json"]}
```

To preserve an existing source while editing, set `destinationPath` on the first mutation. The
engine copies the frozen source into destination-local staging and atomically publishes only the
new file. Apply later mutations to `revised.docx` itself so earlier changes are preserved:

```json
{"operation":"set","path":"source.docx","destinationPath":"revised.docx","arguments":["/body/p[1]","--prop","text=Revised heading","--json"],"reason":"Create a revised copy without overwriting the source"}
```

Render the first page to a workspace PNG. Rendering writes `outputPath`, so it follows the same file-write approval policy as other write tools:

```json
{"operation":"view","path":"report.docx","arguments":["screenshot","--page","1","--json"],"outputPath":"report-preview.png","reason":"Render the finished document for visual inspection"}
```

Validate the OOXML package:

```json
{"operation":"validate","path":"report.docx","arguments":["--json"]}
```

## Create

1. Confirm the requested output name, content hierarchy, page size, and any visual requirements.
2. Create a `.docx` through `office_document`; do not handcraft an OOXML archive or substitute a plain-text file.
3. Apply named styles consistently. Prefer real headings, lists, tables, page breaks, headers, and footers over visual approximations.
4. Render the result and inspect every page for overflow, clipped content, accidental blank pages, weak hierarchy, and inconsistent spacing.
5. Validate the final package and confirm that the requested path exists.

## Edit

1. Inspect the document structure and relevant content before changing it.
2. Preserve styles, relationships, media, sections, and unrelated content unless the user asks to replace them.
3. Prefer targeted operations over rebuilding the whole document.
4. Use `destinationPath` for save-as edits. Omit it only when the user clearly requested in-place editing.
5. Re-inspect and render the result. Verify both the intended change and preservation of surrounding content.

## Quality checks

- Verify headings and reading order, table widths, pagination, margins, headers and footers, and image placement.
- Check that important semantics are represented structurally rather than with repeated spaces or manual line breaks.
- Treat a successful engine exit as necessary but insufficient when visual layout matters.
- If a requested feature is not reported by `help` or validation, describe the limitation instead of simulating support.
- Report only checks that actually ran, including any warnings returned by the tool.
