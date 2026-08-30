"""Receipt-bound compatibility CLI for the managed PDF runtime.

The Host invokes this receipt-covered file with the exact managed Python from an isolated, private
working directory. It intentionally implements only the small command surface documented by the
bundled PDF Skill. Publishable files are confined to ``outputs/``; text intermediates remain inside
the private Run workspace, and inputs are frozen below a Host-validated input root.
"""

from __future__ import annotations

import logging
import math
import os
import sys
import tempfile
from pathlib import Path


MAX_RENDER_PAGES = 32
MIN_RENDER_DPI = 36
MAX_RENDER_DPI = 300
MAX_RENDER_PAGE_PIXELS = 40_000_000
MAX_RENDER_TOTAL_PIXELS = 128_000_000
MAX_EXTRACTED_TEXT_BYTES = 32 * 1024 * 1024
_MANAGED_INPUT_ROOT: Path | None = None


def _expanded_path(value: str) -> Path:
    return Path(os.path.expandvars(value)).expanduser()


def _input_path(value: str) -> Path:
    path = _expanded_path(value)
    if not path.is_absolute():
        path = Path.cwd() / path
    path = path.resolve(strict=True)
    if not path.is_file():
        raise ValueError(f"PDF input is not a regular file: {value}")
    execution_root = Path.cwd().resolve(strict=True)
    allowed_roots = [execution_root]
    if _MANAGED_INPUT_ROOT is not None:
        input_root = _MANAGED_INPUT_ROOT.resolve(strict=True)
        if not input_root.is_dir():
            raise ValueError("MYCOPILOT_INPUT_ROOT is not a directory")
        allowed_roots.append(input_root)
    if not any(_is_below(path, root) for root in allowed_roots):
        raise ValueError("managed PDF inputs must come from the private input or execution root")
    return path


def _is_below(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def _output_path(value: str) -> Path:
    requested = _expanded_path(value)
    if requested.is_absolute():
        candidate = requested.resolve(strict=False)
    else:
        candidate = (Path.cwd() / requested).resolve(strict=False)
    output_root = (Path.cwd() / "outputs").resolve(strict=True)
    try:
        candidate.relative_to(output_root)
    except ValueError as error:
        raise ValueError("managed PDF outputs must remain below outputs/") from error
    candidate.parent.mkdir(parents=True, exist_ok=True)
    # Recheck after creating parents so a path cannot escape through a pre-existing symlink.
    resolved_parent = candidate.parent.resolve(strict=True)
    try:
        resolved_parent.relative_to(output_root)
    except ValueError as error:
        raise ValueError("managed PDF output parent escapes outputs/") from error
    if candidate.exists() and candidate.is_symlink():
        raise ValueError("managed PDF outputs cannot replace symbolic links")
    return candidate


def _intermediate_path(value: str) -> Path:
    requested = _expanded_path(value)
    if requested.is_absolute() or ".." in requested.parts:
        raise ValueError("managed PDF intermediate paths must be relative and cannot contain '..'")
    execution_root = Path.cwd().resolve(strict=True)
    candidate = (execution_root / requested).resolve(strict=False)
    output_root = (execution_root / "outputs").resolve(strict=True)
    try:
        candidate.relative_to(execution_root)
    except ValueError as error:
        raise ValueError("managed PDF intermediate path escapes the private workspace") from error
    try:
        candidate.relative_to(output_root)
    except ValueError:
        pass
    else:
        raise ValueError("text intermediates must be outside outputs/")
    candidate.parent.mkdir(parents=True, exist_ok=True)
    resolved_parent = candidate.parent.resolve(strict=True)
    try:
        resolved_parent.relative_to(execution_root)
    except ValueError as error:
        raise ValueError("managed PDF intermediate parent escapes the private workspace") from error
    if candidate.exists() and candidate.is_symlink():
        raise ValueError("managed PDF intermediates cannot replace symbolic links")
    return candidate


def _reject_encrypted_pdf(path: Path) -> None:
    from pypdf import PdfReader

    reader = PdfReader(str(path))
    if reader.is_encrypted:
        raise ValueError(
            "encrypted PDF requires a password; this managed workflow does not accept passwords"
        )


def _parse_int(args: list[str], index: int, option: str) -> tuple[int, int]:
    if index + 1 >= len(args):
        raise ValueError(f"{option} requires an integer")
    try:
        return int(args[index + 1]), index + 2
    except ValueError as error:
        raise ValueError(f"{option} requires an integer") from error


def _sanitize_extracted_text(text: str) -> str:
    """Keep extracted PDF text searchable by text-oriented pipeline consumers.

    Some valid PDF character maps are surfaced by pypdf as U+0000. Passing that code point
    through UTF-8 produces a literal NUL byte, which makes ripgrep classify the otherwise textual
    stream as binary and suppress its matching lines. Remove only NUL here at the Host-owned text
    producer; all other Unicode, whitespace, page separators, and control characters retain their
    existing meaning.
    """

    return text.replace("\x00", "")


def _pdf_text_chunks(
    path: Path,
    first_page: int,
    last_page: int | None,
    layout: bool,
    page_breaks: bool,
):
    from pypdf import PdfReader

    # Keep non-fatal damaged-xref recovery diagnostics out of model-facing command output.
    # Fatal parse failures still propagate as managed command errors.
    logging.getLogger("pypdf").setLevel(logging.ERROR)
    separator = "\n\f\n" if page_breaks else "\n\n"
    reader = PdfReader(str(path))
    if reader.is_encrypted:
        raise ValueError(
            "encrypted PDF requires a password; this managed workflow does not accept passwords"
        )
    end = len(reader.pages) if last_page is None else min(last_page, len(reader.pages))
    if first_page > end:
        raise ValueError(
            f"first page {first_page} exceeds document page count {len(reader.pages)}"
        )
    for page_number in range(first_page, end + 1):
        page = reader.pages[page_number - 1]
        text = (
            page.extract_text(extraction_mode="layout")
            if layout
            else page.extract_text()
        ) or ""
        source = f"{separator if page_number > first_page else ''}--- Page {page_number} ---\n{text}"
        source_bytes = len(source.encode("utf-8"))
        text = _sanitize_extracted_text(text)
        prefix = separator if page_number > first_page else ""
        yield f"{prefix}--- Page {page_number} ---\n{text}".encode("utf-8"), source_bytes


def pdfinfo(args: list[str]) -> None:
    if len(args) != 1:
        raise ValueError("usage: pdfinfo <input.pdf>")
    from pypdf import PdfReader

    path = _input_path(args[0])
    reader = PdfReader(str(path))
    print(f"Encrypted: {'yes' if reader.is_encrypted else 'no'}")
    if reader.is_encrypted:
        raise ValueError(
            "encrypted PDF requires a password; page metadata cannot be inspected safely"
        )
    print(f"Pages: {len(reader.pages)}")
    if reader.pages:
        media_box = reader.pages[0].mediabox
        width = float(media_box.width)
        height = float(media_box.height)
        print(f"First page size: {width:.2f} x {height:.2f} points")
    metadata = reader.metadata
    if metadata:
        for key in sorted(metadata.keys(), key=str):
            value = metadata.get(key)
            if value not in (None, ""):
                label = str(key).lstrip("/") or "Metadata"
                print(f"{label}: {value}")


def pdftotext(args: list[str]) -> None:
    first_page = 1
    last_page: int | None = None
    layout = False
    page_breaks = True
    positional: list[str] = []
    index = 0
    while index < len(args):
        value = args[index]
        if value == "-f":
            first_page, index = _parse_int(args, index, value)
        elif value == "-l":
            last_page, index = _parse_int(args, index, value)
        elif value == "-layout":
            layout = True
            index += 1
        elif value == "-nopgbrk":
            page_breaks = False
            index += 1
        elif value.startswith("-") and value != "-":
            raise ValueError(f"unsupported pdftotext option: {value}")
        else:
            positional.append(value)
            index += 1
    if len(positional) not in (1, 2):
        raise ValueError(
            "usage: pdftotext [-f page] [-l page] [-layout] [-nopgbrk] <input.pdf> [output.txt|-]"
        )
    if first_page < 1 or (last_page is not None and last_page < first_page):
        raise ValueError("invalid PDF page range")

    path = _input_path(positional[0])
    destination = positional[1] if len(positional) == 2 else "-"
    output = None if destination == "-" else _intermediate_path(destination)
    extracted_pages = 0
    source_bytes = 0
    extracted_bytes = 0
    temporary_path: Path | None = None
    temporary = None
    if output is not None:
        private_temp = Path.cwd() / ".tmp"
        private_temp.mkdir(mode=0o700, exist_ok=True)
        temporary = tempfile.NamedTemporaryFile(
            mode="w+b",
            prefix="pdftotext-",
            suffix=".tmp",
            dir=private_temp,
            delete=False,
        )
        temporary_path = Path(temporary.name)
    writer = temporary if temporary is not None else sys.stdout.buffer
    try:
        for encoded, page_source_bytes in _pdf_text_chunks(
            path, first_page, last_page, layout, page_breaks
        ):
            if source_bytes + page_source_bytes + 1 > MAX_EXTRACTED_TEXT_BYTES:
                raise ValueError(
                    "extracted text exceeds the 32 MiB safety limit; "
                    "narrow the page range with -f and -l"
                )
            writer.write(encoded)
            source_bytes += page_source_bytes
            extracted_bytes += len(encoded)
            extracted_pages += 1
            if temporary is None:
                # Let a bounded downstream reader terminate production before later pages are
                # parsed. BrokenPipeError is normalized by the process entry point below.
                writer.flush()
        writer.write(b"\n")
        extracted_bytes += 1
        writer.flush()

        if temporary is not None:
            os.fsync(temporary.fileno())
            temporary.close()
            os.replace(temporary_path, output)
            temporary_path = None
            print(
                f"Extracted {extracted_pages} page(s), {extracted_bytes} UTF-8 byte(s) "
                f"to {output.relative_to(Path.cwd())}"
            )
    finally:
        if temporary is not None and not temporary.closed:
            temporary.close()
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)


def pdftoppm(args: list[str]) -> None:
    first_page = 1
    last_page: int | None = None
    dpi = 144
    image_format = "png"
    single_file = False
    positional: list[str] = []
    index = 0
    while index < len(args):
        value = args[index]
        if value == "-f":
            first_page, index = _parse_int(args, index, value)
        elif value == "-l":
            last_page, index = _parse_int(args, index, value)
        elif value == "-r":
            dpi, index = _parse_int(args, index, value)
        elif value == "-png":
            image_format = "png"
            index += 1
        elif value in ("-jpeg", "-jpg"):
            image_format = "jpg"
            index += 1
        elif value == "-singlefile":
            single_file = True
            index += 1
        elif value.startswith("-"):
            raise ValueError(f"unsupported pdftoppm option: {value}")
        else:
            positional.append(value)
            index += 1
    if len(positional) != 2:
        raise ValueError(
            "usage: pdftoppm [-f page] [-l page] [-r dpi] [-png|-jpeg] [-singlefile] <input.pdf> outputs/prefix"
        )
    if not MIN_RENDER_DPI <= dpi <= MAX_RENDER_DPI:
        raise ValueError(f"render DPI must be between {MIN_RENDER_DPI} and {MAX_RENDER_DPI}")
    if first_page < 1 or (last_page is not None and last_page < first_page):
        raise ValueError("invalid PDF page range")

    import pypdfium2 as pdfium

    path = _input_path(positional[0])
    _reject_encrypted_pdf(path)
    prefix = _output_path(positional[1])
    document = pdfium.PdfDocument(path)
    end = len(document) if last_page is None else min(last_page, len(document))
    if first_page > end:
        raise ValueError(f"first page {first_page} exceeds document page count {len(document)}")
    count = end - first_page + 1
    if count > MAX_RENDER_PAGES:
        raise ValueError(
            f"one render command supports at most {MAX_RENDER_PAGES} pages; narrow the page range"
        )
    scale = dpi / 72.0
    total_pixels = 0
    for page_number in range(first_page, end + 1):
        width_points, height_points = document[page_number - 1].get_size()
        if (
            not math.isfinite(width_points)
            or not math.isfinite(height_points)
            or width_points <= 0
            or height_points <= 0
        ):
            raise ValueError(f"page {page_number} has invalid dimensions")
        width_pixels = math.ceil(width_points * scale)
        height_pixels = math.ceil(height_points * scale)
        page_pixels = width_pixels * height_pixels
        if page_pixels > MAX_RENDER_PAGE_PIXELS:
            raise ValueError(
                f"page {page_number} would render {page_pixels} pixels, above the "
                f"{MAX_RENDER_PAGE_PIXELS} single-page safety limit; lower -r"
            )
        total_pixels += page_pixels
        if total_pixels > MAX_RENDER_TOTAL_PIXELS:
            raise ValueError(
                f"the requested page range would render more than "
                f"{MAX_RENDER_TOTAL_PIXELS} total pixels; lower -r or narrow -f/-l"
            )
    generated: list[Path] = []
    for page_number in range(first_page, end + 1):
        page = document[page_number - 1]
        image = page.render(scale=scale).to_pil()
        suffix = prefix.suffix
        stem = prefix.with_suffix("") if suffix else prefix
        output = (
            stem.with_suffix(f".{image_format}")
            if single_file and count == 1
            else Path(f"{stem}-{page_number}.{image_format}")
        )
        output = _output_path(str(output))
        image.save(output, format="PNG" if image_format == "png" else "JPEG")
        generated.append(output)
    for output in generated:
        print(f"Generated {output.relative_to(Path.cwd())}")


def main() -> None:
    global _MANAGED_INPUT_ROOT

    arguments = sys.argv[1:]
    if len(arguments) >= 2 and arguments[0] == "--managed-input-root":
        _MANAGED_INPUT_ROOT = Path(arguments[1])
        arguments = arguments[2:]
    if not arguments:
        raise ValueError("managed PDF command name is required")
    command, args = arguments[0], arguments[1:]
    if command == "pdfinfo":
        pdfinfo(args)
    elif command == "pdftotext":
        pdftotext(args)
    elif command == "pdftoppm":
        pdftoppm(args)
    else:
        raise ValueError(f"unsupported managed PDF command: {command}")


if __name__ == "__main__":
    try:
        main()
    except BrokenPipeError:
        # A bounded downstream reader such as `rg --max-count` may intentionally stop before
        # pdftotext finishes. Treat that normal pipeline backpressure as success and replace
        # stdout so Python's interpreter-shutdown flush cannot emit a second BrokenPipeError.
        devnull = os.open(os.devnull, os.O_WRONLY)
        try:
            os.dup2(devnull, sys.stdout.fileno())
        finally:
            os.close(devnull)
        raise SystemExit(0)
    except Exception as error:
        print(f"managed PDF command failed: {error}", file=sys.stderr)
        raise SystemExit(2)
