# Creating and editing

## Create a safe output

Create a new PDF by default. Preserve the source and use a descriptive filename unless the user explicitly requests replacement. Warn before changing a signed document.

Use managed `reportlab` for new PDFs, `pypdf` for assembly, metadata, transformations, overlays, and bounded structural edits, and `pdfplumber` for inspection. Bind source PDFs, images, fonts, and data through `run_command.inputs`. Write final PDFs and rendered QA images below `outputs/`; keep unpublished intermediates private.

For short logic, use one quoted heredoc rather than compressed one-line Python:

```sh
python - <<'PY'
from reportlab.lib.pagesizes import A4
from reportlab.pdfgen import canvas

output = "outputs/summary.pdf"
pdf = canvas.Canvas(output, pagesize=A4)
pdf.drawString(72, 770, "Summary")
pdf.save()
print(output)
PY
```

Keep stdout to a short status or bounded validation report. Do not install packages, execute downloaded code, or write outside the private execution space and `outputs/` publication boundary.

## Build predictably

- Set page size, margins, typography, line spacing, and pagination explicitly.
- Keep tables inside the printable region; repeat headers and use readable widths.
- Preserve image aspect ratios and adequate resolution.
- Use fonts that cover required glyphs.
- Avoid clipping, overlap, replacement boxes, accidental blank pages, and orphaned headings.
- Preserve metadata and links when required.

## Validate before delivery

1. Reopen the result with `pypdf`; fail on parser errors.
2. Run `pdfinfo` and verify page count, dimensions, encryption state, and relevant metadata.
3. Extract a bounded sample or changed page range and confirm requested content.
4. Render every changed page, or every page for a small document.
5. Pass each returned image `readPath` to `read_image` and inspect margins, clipping, overlap, fonts, images, tables, page order, headers, footers, and blanks.
6. Fix defects and repeat validation.

Deliver only the exact published Artifact `readPath`. If a structural or visual check could not be completed, name the missing check.
