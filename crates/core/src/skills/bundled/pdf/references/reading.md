# Reading and inspection

## Establish structure

Inspect an existing workspace PDF by its exact relative path:

```sh
pdfinfo "manuals/manual.pdf"
```

For a declared input, use its mount below the input root:

```sh
pdfinfo "$MYCOPILOT_INPUT_ROOT/manual.pdf"
```

Record the one-based page count first. Note encryption, dimensions, title, or author only when relevant.

The managed shell exposes exactly `pdfinfo`, `pdftotext`, `pdftoppm`, `python`, `python3`, and `rg`. It does not expose `head`, `tail`, `grep`, `sed`, `awk`, or other ambient shell programs.

## Locate before loading

For a large text PDF, stream extraction into the receipt-bound `rg` and cap matches:

```sh
pdftotext -layout "$MYCOPILOT_INPUT_ROOT/manual.pdf" - \
  | rg -n -i -C 4 --max-count 20 "steady[- ]state|unit operation"
```

Use distinctive headings, section numbers, captions, or terms. If 20 matches are ambiguous, refine the expression rather than increasing output without a reason. Once the relevant pages are known, extract them directly:

To inspect only the first `N` extracted lines without `head`, match every line and cap the receipt-bound `rg`:

```sh
pdftotext -layout "$MYCOPILOT_INPUT_ROOT/manual.pdf" - \
  | rg --max-count 80 '^'
```

This limits extracted-text lines, not PDF pages. For a known page interval, use the page-bounded form instead:

`rg -n` reports extracted-text line numbers, not PDF page numbers. When the bounded context does not include a nearby `--- Page N ---` marker, use the page-aware Python fallback below to map the term to PDF pages; do not translate line numbers into byte-offset slicing.

```sh
pdftotext -f 42 -l 46 -layout "$MYCOPILOT_INPUT_ROOT/manual.pdf" -
```

Read enough adjacent pages to capture section boundaries, continuations, footnotes, captions, and sources. Do not dump the full text of a large document and then issue repeated Python commands that slice the same stdout by byte or character offset. If a command result exceeded the model budget, use its `historyOpen` with `conversation_history` instead of repeating extraction; `head`, `tail`, and byte-offset slicing are not recovery paths.

Prefer `pdftotext` for fast layout-aware text, `pypdf` for page-level structure, and `pdfplumber` for positional text and tables. If a tool produces empty or clearly damaged text, switch tools once or render the relevant pages; do not repeat the same failing command.

For bounded page-aware logic that the CLI cannot express, use a quoted heredoc and print only selected evidence:

```sh
python - "$MYCOPILOT_INPUT_ROOT/manual.pdf" <<'PY'
import re, sys
from pypdf import PdfReader

reader = PdfReader(sys.argv[1])
pattern = re.compile(r"steady[- ]state|unit operation", re.I)
shown = 0
for page_number, page in enumerate(reader.pages, 1):
    text = page.extract_text() or ""
    if pattern.search(text):
        excerpt = " ".join(text.split())[:1200]
        print(f"PAGE {page_number}: {excerpt}")
        shown += 1
        if shown == 12:
            break
PY
```

Keep explicit match, page, and excerpt limits. A heredoc is one command; do not create an executable temporary script just to work around quoting.

## Render only relevant pages

Text matches are not visual evidence. Render a bounded page range into publishable outputs:

```sh
pdftoppm -f 42 -l 46 -png "$MYCOPILOT_INPUT_ROOT/manual.pdf" outputs/manual-page
```

Pass every returned image `readPath` unchanged to `read_image`. Inspect headings, column order, tables, diagrams, captions, footnotes, handwriting, stamps, clipping, and unreadable glyphs. For scanned PDFs or pages with unusable extraction, use this visual path immediately.

## Answering gate

- Name the relevant PDF page numbers.
- Preserve material labels, units, dates, qualifications, sources, and sample sizes.
- Distinguish document statements from inference.
- Disclose unreadable or unverified content instead of guessing.
