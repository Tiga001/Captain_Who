"""Archive this task's synthetic text evidence without changing its cache originals.

Use --dry-run to validate and calculate the complete plan without writing any file.
Future round6 run directories are discovered at invocation, but only the three named
runner outputs are selected. No recursive scan of build targets or database fixtures.
"""

import argparse
from collections import Counter
from datetime import datetime, timezone
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import tempfile


ROOT = next(p for p in Path(__file__).resolve().parents if (p / "Cargo.toml").is_file())
CACHE = ROOT / ".cache/backend-concurrency"
SOURCE = CACHE / "round6-validation-20261011"
DEST = ROOT / "docs/performance/backend-concurrency/results/2026-10-12-round6"
ARCHIVE_ID = "2026-10-12-round6-synthetic-evidence"
FORMAT = "round6-synthetic-evidence-transform-v1"
ALLOW = {".json", ".jsonl", ".log", ".txt", ".md", ".py", ".c", ".patch", ".csv", ".sha256"}
RUN_FILES = {"cases.jsonl", "metadata.json", "resources.jsonl"}
VALIDATION_TREES = {"storage-final-evidence", "regressions", "diagnostics", "fts-crash-investigation"}
SKIP_DIRECTORIES = {"target", "binaries", "node_modules", ".git", "__pycache__"}
HASH_NAMES = {"SHA256SUMS", "frontend-SHA256SUMS", "final-matrix-review-SHA256SUMS", "residual-hotspots-SHA256SUMS"}
INTERIM_DRAFTS = {"round6-results-draft.md", "round6-results-resume-draft.md", "delivery-checklist.json"}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def require_no_symlink(path):
    """Check each path component, including excluded files encountered in selected trees."""
    for candidate in (path, *path.parents):
        if candidate.is_symlink():
            raise ValueError(f"symlink refused: {candidate.relative_to(ROOT)}")
        if candidate == ROOT:
            break
    if not path.resolve().is_relative_to(ROOT):
        raise ValueError("evidence path escapes repository")


def identity(path):
    require_no_symlink(path)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode):
        raise ValueError(f"not a regular file: {path.relative_to(ROOT)}")
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns)


def allowed(path):
    # Directory policy supplies provenance; suffix checks are an additional binary/DB gate.
    return path.suffix in ALLOW or path.name in HASH_NAMES


def walk_text_tree(directory):
    require_no_symlink(directory)
    if not directory.exists():
        return
    for path in sorted(directory.iterdir()):
        require_no_symlink(path)
        if path.is_dir():
            if path.name not in SKIP_DIRECTORIES:
                yield from walk_text_tree(path)
        elif allowed(path):
            identity(path)
            yield path


def select_files():
    """Materialize an explicit source -> archive list before reading or publishing."""
    require_no_symlink(CACHE)
    require_no_symlink(SOURCE)
    selected = {}
    for path in sorted(SOURCE.iterdir()):
        require_no_symlink(path)
        if path.is_file() and allowed(path) and path.name not in INTERIM_DRAFTS:
            selected[path] = Path("validation") / path.name
    for name in sorted(VALIDATION_TREES):
        for path in walk_text_tree(SOURCE / name):
            selected[path] = Path("validation") / path.relative_to(SOURCE)
    # No fixed r1/r2/r3 list: future complete or failed runs remain eligible by these names.
    for directory in sorted(CACHE.iterdir()):
        if directory == SOURCE or not directory.name.startswith("round6-"):
            continue
        require_no_symlink(directory)
        if directory.is_dir():
            for name in sorted(RUN_FILES):
                path = directory / name
                require_no_symlink(path)
                if path.exists():
                    identity(path)
                    selected[path] = Path("runs") / directory.name / name
    return sorted(selected.items(), key=lambda pair: str(pair[1]))


def path_rules():
    # Literal rules run longest first; replace repository/worktree before the enclosing home.
    literals = [(str(ROOT), "<repo>", "repository_root")]
    home = Path.home()
    if home != Path("/"):
        literals.append((str(home), "<home>", "user_home"))
        # This task's actual managed baseline checkout also appears in retained build metadata.
        baseline = home / ".codex/worktrees/round6-baseline" / ROOT.name
        for value in {str(baseline), str(baseline.resolve())}:
            literals.append((value, "<baseline-worktree>", "baseline_worktree"))
    for prefix, label, placeholder in [
        ("captain-backend-baseline-*-target", "isolated_build", "<build>"),
        ("mycopilot-head-baseline.*", "baseline_worktree", "<baseline-worktree>"),
        ("captain-backend-baseline-*-worktree", "baseline_worktree", "<baseline-worktree>"),
    ]:
        for path in Path("/tmp").glob(prefix):
            if path.is_dir():
                for value in {str(path), str(path.resolve())}:
                    literals.append((value, placeholder, label))
    literals = sorted(set(literals), key=lambda rule: len(rule[0]), reverse=True)
    # Also cover retained commands after their isolated build/worktree directories are removed.
    patterns = [
        (r"/(?:private/)?tmp/captain-backend-baseline-[A-Za-z0-9._-]+-target(?=/|[\s\"'`\\:]|$)", "<build>", "isolated_build"),
        (r"/(?:private/)?tmp/mycopilot-head-baseline\.[A-Za-z0-9_-]+(?=/|[\s\"'`\\:]|$)", "<baseline-worktree>", "baseline_worktree"),
        (r"/(?:private/)?tmp/captain-backend-baseline-[A-Za-z0-9._-]+-worktree(?=/|[\s\"'`\\:]|$)", "<baseline-worktree>", "baseline_worktree"),
    ]
    return literals, patterns


def transform(data, rules, source):
    try:
        text = data.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise ValueError(f"non-UTF8 refused: {source.relative_to(ROOT)}") from error
    if "\0" in text:
        raise ValueError(f"NUL/binary content refused: {source.relative_to(ROOT)}")
    counts = Counter()
    literals, patterns = rules
    for original, replacement, label in literals:
        count = text.count(original)
        if count:
            text = text.replace(original, replacement)
            counts[label] += count
    for pattern, replacement, label in patterns:
        text, count = re.subn(pattern, replacement, text)
        counts[label] += count
    # Reject unknown real-looking user roots instead of silently exporting unreviewed paths.
    if re.search(r"/(?:Users|home)/[^/\s\"'<>]+", text):
        raise ValueError(f"unmapped user-home path refused: {source.relative_to(ROOT)}")
    return text.encode("utf-8"), dict(sorted((k, v) for k, v in counts.items() if v))


def compress(data):
    output = io.BytesIO()
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as handle:
        handle.write(data)
    return output.getvalue()


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def check_destination(replace_existing):
    require_no_symlink(DEST)
    if not DEST.exists():
        return
    if not replace_existing:
        raise ValueError("round6 destination already exists; use --replace-existing only for this archive")
    manifest = DEST / "TRANSFORM-MANIFEST.json"
    require_no_symlink(manifest)
    if not manifest.is_file():
        raise ValueError("refusing to replace a destination without this archive's ownership manifest")
    recorded = json.loads(manifest.read_text(encoding="utf-8"))
    if recorded.get("format") != FORMAT or recorded.get("archive_id") != ARCHIVE_ID:
        raise ValueError("refusing to replace an unrelated destination")
    for path in DEST.rglob("*"):
        require_no_symlink(path)


def publish(stage, replace_existing):
    check_destination(replace_existing)
    previous = None
    if DEST.exists():
        previous = Path(tempfile.mkdtemp(prefix=".round6-previous-", dir=DEST.parent))
        previous.rmdir()
        os.replace(DEST, previous)
    try:
        os.replace(stage, DEST)
    except BaseException:
        if previous is not None:
            os.replace(previous, DEST)
        raise
    if previous is not None:
        shutil.rmtree(previous)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="validate/transform/hash in memory; write nothing")
    parser.add_argument("--replace-existing", action="store_true", help="replace only a prior archive owned by this script")
    args = parser.parse_args()
    check_destination(args.replace_existing)
    selected = select_files()
    originals = {path: identity(path) for path, _ in selected}
    rules = path_rules()
    stage = None
    if not args.dry_run:
        DEST.parent.mkdir(parents=True, exist_ok=True)
        stage = Path(tempfile.mkdtemp(prefix=".round6-stage-", dir=DEST.parent))
    entries, checksums, names, transformations = [], [], set(), Counter()
    try:
        for source, relative in selected:
            data = source.read_bytes()
            if identity(source) != originals[source]:
                raise ValueError(f"source changed during read: {source.relative_to(ROOT)}")
            text, changed = transform(data, rules, source)
            # Historical Markdown is evidence text, not maintained documentation.
            if relative.suffix == ".md":
                relative = relative.with_name(relative.name + ".txt")
            encoded = compress(text) if len(text) > 65536 else text
            if len(text) > 65536:
                relative = relative.with_name(relative.name + ".gz")
            name = relative.as_posix()
            if name in names:
                raise ValueError(f"duplicate archive destination: {name}")
            names.add(name)
            transformations.update(changed)
            entries.append({
                "source_path": source.relative_to(ROOT).as_posix(),
                "source_bytes": len(data), "source_sha256": sha(data),
                "archive_path": name, "archive_bytes": len(encoded), "archive_sha256": sha(encoded),
                "archived_utf8_bytes": len(text), "archived_utf8_sha256": sha(text),
                "compression": "gzip-mtime-0" if len(text) > 65536 else "none",
                "path_transformations": changed,
            })
            checksums.append((name, sha(encoded)))
            if stage is not None:
                target = stage / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(encoded)
        if selected != select_files() or any(identity(path) != before for path, before in originals.items()):
            raise ValueError("evidence inventory changed during archival; rerun after writers stop")
        manifest = {
            "format": FORMAT, "archive_id": ARCHIVE_ID,
            "created_at": datetime.now(timezone.utc).isoformat(),
            "source_scope": "Only this task's synthetic validation text and named round6 runner outputs; cache originals are never modified.",
            "selection_policy": {"validation_top_level_suffixes": sorted(ALLOW), "validation_subtrees": sorted(VALIDATION_TREES), "run_directory_prefix": "round6-", "run_top_level_files": sorted(RUN_FILES), "excluded_directories": sorted(SKIP_DIRECTORIES), "excluded_interim_drafts": sorted(INTERIM_DRAFTS), "historical_markdown_extension": ".md.txt"},
            "transformations": {
                "repository_root": "Local repository absolute paths become <repo>.",
                "isolated_build": "Task's isolated Cargo target paths become <build>.",
                "user_home": "Current user's home-directory prefix becomes <home>.",
                "baseline_worktree": "Task's baseline worktree absolute paths become <baseline-worktree>.",
            },
            "embedded_hash_policy": "All hashes/counts/times already present inside copied metadata, logs and old manifests remain unchanged and refer to their ORIGINAL inputs, not necessarily to transformed archive bytes. Use this manifest's archive_sha256 and top-level SHA256SUMS for the exported files. Only declared path-string replacements and deterministic gzip compression are applied.",
            "entries": entries,
        }
        readme = b"---\nstatus: historical\naudience: developers/maintainers\nowner: engineering\nlast_verified: 2026-10-12\n---\n\n# Synthetic round6 evidence archive\n\nStart with ../../round6-results.md and validation/final-delivery-summary.json for the final verdict. Historical Markdown is stored as .md.txt to keep source evidence distinct from maintained documentation. Historical logs and analyses retain their original findings, including rejected measurements; they are not independent acceptance claims.\n\nCache originals remain unchanged. Only path strings are redacted; numbers, test outcomes, failures and timestamps are preserved. See TRANSFORM-MANIFEST.json for every source/archived SHA256 and transformation. Existing hash fields and nested SHA256SUMS in copied evidence still refer to ORIGINAL files; top-level SHA256SUMS and archive_sha256 refer to this transformed archive. No executable, database, WAL/SHM, build target, credential store, screenshot or real user data root is selected. This is evidence, not a database backup.\n"
        generated = {"TRANSFORM-MANIFEST.json": json_bytes(manifest), "README.md": readme}
        for name, data in generated.items():
            checksums.append((name, sha(data)))
            if stage is not None:
                (stage / name).write_bytes(data)
        sums = "".join(f"{digest}  {name}\n" for name, digest in sorted(checksums)).encode("utf-8")
        if stage is not None:
            (stage / "SHA256SUMS").write_bytes(sums)
            # Verify the new staging only. Existing destination contents never enter this list.
            for name, digest in checksums:
                if sha((stage / name).read_bytes()) != digest:
                    raise ValueError(f"staging hash mismatch: {name}")
            publish(stage, args.replace_existing)
            stage = None
        print(json.dumps({
            "dry_run": args.dry_run, "destination": DEST.relative_to(ROOT).as_posix(),
            "source_files": len(entries), "archive_files": len(entries) + 3,
            "source_bytes": sum(e["source_bytes"] for e in entries),
            "archive_bytes": sum(e["archive_bytes"] for e in entries) + sum(map(len, generated.values())) + len(sums),
            "transformation_counts": dict(sorted(transformations.items())),
            "run_directories": sorted({Path(e["source_path"]).parent.name for e in entries if e["archive_path"].startswith("runs/")}),
            "writes": "none" if args.dry_run else "new independent staging published to the dedicated round6 directory",
        }, ensure_ascii=False))
    finally:
        if stage is not None and stage.exists():
            shutil.rmtree(stage)


if __name__ == "__main__":
    main()
