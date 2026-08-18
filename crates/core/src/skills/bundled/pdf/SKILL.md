---
name: pdf
description: Read, search, inspect, create, edit, render, and verify PDF files, including scanned documents, complex layouts, tables, figures, and fillable forms. Use whenever the task involves a .pdf file or PDF output.
---

# PDF

Use only `run_command` for PDF processing; use `read_image` for visual inspection. The complete executable set is exactly `pdfinfo`, `pdftotext`, `pdftoppm`, `python`, `python3`, and `rg`. Commands such as `head`, `tail`, `grep`, `sed`, and `awk` are unavailable; never install replacements or fall back to system tools.

For a PDF already in the selected workspace, use its exact safe workspace-relative path. This
includes the exact workspace-relative PDF `readPath` returned by `office_document.render` for a QA
PDF outside top-level `outputs/`: pass it unchanged, without `run_command.inputs` or a
`MYCOPILOT_INPUT_ROOT` rewrite. For an attachment, external file, generated Artifact, Skill
resource, script, image, font, or other source, keep using `run_command.inputs`, then use its mounted
path below `MYCOPILOT_INPUT_ROOT`. Never guess a physical attachment, Artifact, runtime, or private
working-directory path.

Reserve the managed PDF command's top-level `outputs/` directory for files that command creates,
such as page images. Never use `outputs/` as an input alias or reconstruct an input path there; use
the exact source path instead.

## Core workflow

1. Run `pdfinfo` to learn page count, encryption state, page size, and relevant metadata.
2. For a large text PDF, locate a section with bounded streaming search before extracting pages:

   ```sh
   pdftotext -layout "manual.pdf" - | rg -n -i -C 4 --max-count 20 "steady|unit operation"
   ```

   To inspect only the first `N` extracted lines, use the receipt-bound `rg` instead of `head`:

   ```sh
   pdftotext -layout "manual.pdf" - | rg --max-count 80 '^'
   ```

3. Extract only the matching page range plus necessary adjacent pages with `pdftotext -f FIRST -l LAST`. Use `rg --max-count N` to bound matches or initial lines. Do not print an entire large PDF, repeatedly slice the same full extraction by output offsets, or rerun a command whose archived result is available through `historyOpen`; open that exact result with `conversation_history` instead.
4. Use text extraction for meaning, not visual proof. Render relevant pages and call `read_image` for scans, tables, figures, columns, forms, or layout questions. If text is empty or damaged, switch to page rendering.
5. After creating or editing, reopen the PDF, validate its structure and requested content, render all changed pages (or every page when small), and inspect those images before delivery.
6. State which pages were actually extracted or rendered; never claim coverage you did not inspect.

Use shell pipelines and redirection only to connect this fixed managed toolchain or create bounded private intermediates. Use a quoted Python heredoc for short fallback logic, and keep its output explicitly bounded. Do not download packages, invoke `pip`, Homebrew, or `apt`, discover the runtime, or expose private paths.

Read the relevant reference before acting:

- [references/reading.md](references/reading.md) for search, page selection, extraction, and visual review.
- [references/creating-and-editing.md](references/creating-and-editing.md) for generation, modification, validation, and delivery.
- [references/forms.md](references/forms.md) for AcroForm inspection, filling, appearance validation, and flattening.
