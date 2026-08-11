---
name: pdf
description: Read, search, inspect, create, edit, render, and verify PDF files, including scanned documents, complex layouts, tables, figures, and fillable forms. Use whenever the task involves a .pdf file or PDF output.
---

# PDF

Use only `run_command` for PDF processing and `read_image` for visual inspection. No dedicated PDF model tool is exposed.

For an existing PDF in the selected workspace, pass its exact safe workspace-relative path directly to `pdfinfo`, `pdftotext`, or `pdftoppm`; the Host freezes and mounts that explicit PDF input automatically. Bind every attachment, external file, generated Artifact, Skill resource, Python script, and additional source through `run_command.inputs`. For a declared input, use the exact user path or returned `readPath`, choose an optional safe `mountPath`, and read it below `MYCOPILOT_INPUT_ROOT`. Never open an `@attachments/...`, `artifact://...`, or `skill://...` value directly from a command.

Use the managed commands and Python environment supplied after this trusted Skill is activated. Omit `runtimeProfile`; never use system Python, install packages, or invoke `pip`, Homebrew, `apt`, or another dependency manager. If the managed PDF capability is unavailable, stop with the returned recovery guidance instead of falling back to the machine environment.

Use one direct managed invocation per `run_command`: `pdfinfo`, `pdftotext`, `pdftoppm`, `python -c`, or a frozen input-root Python script. Pipelines, `&&`, `||`, `;`, shell redirection, and heredocs are intentionally unavailable inside this managed PDF surface. For temporary bounded logic, put literal newlines inside the quoted `python -c` code argument; do not wrap it in `sh`, write a temporary script, or use a heredoc. A reusable script must already be an authorized file: bind it with `run_command.inputs` and invoke it only below `MYCOPILOT_INPUT_ROOT`; never execute a saved script from the private execution space. Write unpublished intermediate text and other non-executable data to safe relative names there, such as `extracted.txt`; use stdout when a saved intermediate is unnecessary. Write only publishable PNG, JPEG, WebP, or PDF files below relative `outputs/...` paths. Intermediate files do not receive a `readPath`. Never discover, infer, expose, or report a physical working directory. After a command publishes files, copy its exact `outputs[].readPath`; pass an image `readPath` unchanged to `read_image.path` and use the returned Artifact path unchanged for delivery. Do not reconstruct a path from the command, stdout, or a file search.

## Required workflow

1. Run `pdfinfo` before reading to establish page count, metadata, encryption state, and page dimensions.
2. For a large PDF, locate the relevant keyword, section, or page range before extracting or rendering. Do not emit or render the whole document by default.
3. Use `pdftotext`, `pypdf`, or `pdfplumber` for semantic extraction. Text extraction does not prove layout, table geometry, figure content, or visual correctness.
4. Render every relevant page for scanned content, tables, figures, complex layout, or any visual question, then inspect every returned image with `read_image`. If extraction is empty or damaged, switch to rendered-page inspection.
5. After creating or editing, reopen and validate the written PDF, render all changed pages (or every page for a small document), and inspect them before delivery.
6. State which pages were actually extracted or rendered. Never claim to have read an uninspected page.

If command output crosses the shared model-result budget, use its `historyOpen` reference with `conversation_history`; do not rerun the same command merely to recover archived output.

Read the reference that matches the task:

- [references/reading.md](references/reading.md) for search, extraction, page selection, and visual review.
- [references/creating-and-editing.md](references/creating-and-editing.md) for generation, modification, validation, and delivery.
- [references/forms.md](references/forms.md) for AcroForm inspection, filling, appearance validation, and flattening.
