# Creating and editing

## Choose a safe output

Create a new PDF by default. Preserve the source and use a distinct descriptive filename unless the user explicitly requests an in-place replacement and current permissions allow it. Never overwrite a signed PDF without an explicit decision about signature invalidation.

Use `reportlab` to generate new documents. Use `pypdf` for page assembly, metadata, transformations, overlays, and bounded structural edits; use `pdfplumber` for inspection and extraction. Run only the managed `python` and pinned libraries supplied by the activated Skill.

Use managed `python -c` for short generation or edit logic, including a quoted code argument with literal newlines when several statements are needed. It remains one direct managed invocation: do not use a shell wrapper, heredoc, pipeline, or compound shell operator. For reusable logic, first save the script as an authorized workspace or local file, bind that file with `run_command.inputs`, and invoke the frozen script only below `MYCOPILOT_INPUT_ROOT`. Never create and later execute a script from the private execution space. Bind source PDFs, images, fonts, and data the same way. Use the private space only for non-script intermediates such as extracted text; write only publishable PDF or rendered images below `outputs/`.

## Build for predictable rendering

- Set page size, margins, typography, line spacing, and pagination explicitly.
- Keep tables within the printable region; repeat headers and preserve readable column widths.
- Preserve image aspect ratios and use adequate resolution.
- Embed or choose fonts that cover every required glyph.
- Avoid clipped text, overlaps, black replacement boxes, accidental blank pages, and orphaned headings.
- Preserve document metadata and links when the task requires them.

## Validate after every meaningful change

1. Reopen the result with `pypdf` and fail on parser errors.
2. Run `pdfinfo` on the result and verify page count, dimensions, encryption state, and expected metadata.
3. Re-extract representative text or structured content and confirm the requested changes are present and unrelated content remains intact.
4. Render every changed page. For a small document, render every page.
5. Pass each exact image `outputs[].readPath` to `read_image` and inspect margins, clipping, overlap, fonts, images, tables, page order, headers, footers, and blank pages.
6. Correct defects and repeat reopening, rendering, and visual inspection. A successful write or text extraction is not final validation.

Deliver only the exact Artifact `readPath` returned after publication. If structural or visual validation could not be completed, say which check is missing rather than claiming success.
