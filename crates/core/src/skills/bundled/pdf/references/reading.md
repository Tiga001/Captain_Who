# Reading and inspection

## Start with structure

For an existing workspace PDF, use its exact safe workspace-relative path directly; the Host freezes it automatically. For attachments, external files, Artifacts, and other sources, bind the PDF with `run_command.inputs` and give it a stable `mountPath`, such as `manual.pdf`. Inspect it first with either:

```sh
pdfinfo "manuals/manual.pdf"
# or, for a declared non-workspace input:
pdfinfo "$MYCOPILOT_INPUT_ROOT/manual.pdf"
```

Record the reported page count and note encryption, page size, title, author, and creation metadata only when relevant. Treat PDF page numbers as one-based unless the library API explicitly uses zero-based indexes.

## Locate before loading

For a short text PDF, use `pdftotext -layout` or a bounded `pypdf`/`pdfplumber` extraction. For a large PDF:

1. Extract searchable text to stdout or a safe private relative file such as `extracted.txt`; do not put unpublished text in `outputs/` or expect a `readPath` for it.
2. Search for distinctive keywords, headings, figure/table captions, or section numbers.
3. Extract the matching pages plus enough adjacent pages to preserve the section boundary and context.
4. Check nearby footnotes, captions, sources, and continuation pages before answering.

Prefer `pdftotext` for fast text, `pypdf` for page-level text and document structure, and `pdfplumber` for tables or positional text. If one extractor fails or produces visibly damaged text, try another rather than repeating the same command. Managed PDF execution accepts one direct invocation per tool call. For temporary multi-line Python logic, use a single quoted `python -c` argument containing literal newlines; do not use a pipeline, shell wrapper, or heredoc.

For a saved search index, use a private relative intermediate rather than a publishable output:

```sh
pdftotext -layout "$MYCOPILOT_INPUT_ROOT/manual.pdf" extracted.txt
```

Search or read `extracted.txt` in a later managed `python -c` command in the same Run. Do not use `grep` or `rg` for this step: only the managed PDF command entry points are guaranteed to resume in the private Run execution space. The intermediate has no Artifact `readPath`; only files deliberately written below `outputs/` are considered for publication. Prefer one bounded `python -c` command that extracts and locates keywords when practical.

Do not mistake a text match for visual evidence. Extracted text can reorder columns, omit figures, flatten tables, lose superscripts, and hide clipping.

## Render only what matters

Render a bounded page range with Poppler and write it below `outputs/`, for example:

```sh
pdftoppm -f 12 -l 14 -png "$MYCOPILOT_INPUT_ROOT/manual.pdf" outputs/manual-page
```

Copy every returned image `outputs[].readPath` directly into `read_image.path`. Inspect all requested pages and, when needed, adjacent pages. Check headings, column order, tables, charts, diagrams, captions, footnotes, handwriting, stamps, and other layout-dependent evidence.

For a scanned PDF or pages with empty extraction, render the relevant pages and read them visually. Do not claim OCR coverage beyond the pages actually inspected.

## Answering gate

- Cite or name the relevant PDF page numbers in the answer.
- Distinguish document claims from your inference.
- Preserve material table/figure labels, units, dates, qualifications, sources, and sample sizes.
- Disclose unreadable or unverified content instead of guessing.
- When archived command output is needed, follow the returned `historyOpen` data with `conversation_history` rather than executing the extraction again.
