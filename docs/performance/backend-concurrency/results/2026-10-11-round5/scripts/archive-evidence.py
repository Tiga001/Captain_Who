#!/usr/bin/env python3
"""Round 5 evidence archiver. Review selections, then explicitly pass --write.

No tests/builds/subprocesses, source edits, deletions, or existing-path overwrites.
Missing/incomplete final evidence fails before any destination is created. A failed
copy leaves its newly created archive intact for inspection; it never removes it.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import sys

VALIDATION_NAME = "round5-validation-20261011"
MATRIX_NAME = "round5-full-20261011-r1"
ARCHIVE_NAME = "2026-10-11-round5"
COMPRESS_THRESHOLD = 64 * 1024

# Explicit successful selections, never guessed from mtime or an arbitrary "last" log.
# If final validation is rerun under another name, review/update this allowlist.
ACCEPTED = (
    "renderer-authority-accepted.log",
    "renderer-authority-unit-fixed.log",
    "renderer-recovery-unit-fixed.log",
    "renderer-authority-typecheck-accepted.log",
    "renderer-authority-lint-accepted.log",
    "refresh-unit-final.log", "refresh-unit-final.json",
    "refresh-browser-final.log", "refresh-browser-final.json",
    "collaboration-typecheck.log",
    "host-unit-final.log", "host-typecheck-node-final.log",
    "host-eslint-final.log", "host-checks-final.json",
    "clippy-accepted.log", "rustfmt.log", "prettier-ts.log",
    "docs-format.log", "check-docs.log", "test-layout.log", "test-infrastructure.log",
    "recovery-barrier-repeat.log", "recovery-barrier-repeat.json",
)
HOST = ("host-read-results.json", "host-read-results.log", "host-read-receipt.json")
BUILD_RECORDS = (
    "accepted-build.stdout.log", "accepted-build.stderr.log",
    "release-final-build.stdout.log", "release-final-build.stderr.log",
    "production-final-build.stdout.log", "production-final-build.stderr.log",
)
EARLY_CHECKPOINT = (
    "checkpoint-first.stdout.log", "checkpoint-first.stderr.log", "checkpoint-second.log",
    "checkpoint-final-runner.log", "runtime_checkpoints-tests-first.log",
    "skill_installation_projection-tests-first.log", "checkpoint_web_read_tests-first.log",
    "file_change_projection-tests-first.log", "read_tool_file_context-tests-first.log",
)
EARLY_FAILURES = (
    "host-read-initial-fixture-failure.log", "renderer-authority-first.log",
    "renderer-authority-second.log", "renderer-authority-unit.log",
    "renderer-recovery-unit.log", "renderer-typecheck-initial.log",
    "renderer-lint-initial.log", "renderer-metadata-dedup-browser.log",
    "clippy.log", "clippy-second.log", "runtime-isolation-first.log",
)
AUDITS = (
    "host-read-audit.md", "renderer-round5-handoff.md", "renderer-persistence-audit.md",
    "renderer-activity-recovery-contract.md", "collaboration-refresh-audit.txt",
    "checkpoint-review.md", "implementation-files.json",
)
REFRESH_INPUTS = (
    "collaboration-refresh-results.json", "benchmark-collaboration-refresh.cjs",
    "baseline-useCollaborationApprovals.ts", "baseline-useObserverConversation.ts",
    "refresh.browser.test.tsx", "refresh-browser.config.ts", "refresh-browser.log",
)
RENDERER_INPUTS = (
    "renderer-serialization-baseline.json", "renderer-serialization-baseline.log",
    "renderer-serialization-baseline.mjs", "renderer-chain-baseline.log",
)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def stable_stat(path: Path) -> tuple[int, int]:
    stat = path.stat()
    return stat.st_size, stat.st_mtime_ns


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path)
    parser.add_argument("--validation", type=Path)
    parser.add_argument("--matrix", type=Path)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--expected-suites", type=int, default=43)
    parser.add_argument("--write", action="store_true", help="publish the reviewed archive")
    args = parser.parse_args()
    repo = (args.repo or next(p for p in Path(__file__).resolve().parents
                             if (p / "Cargo.toml").is_file())).resolve()
    validation = (args.validation or repo / ".cache/backend-concurrency" / VALIDATION_NAME).resolve()
    matrix = (args.matrix or repo / ".cache/backend-concurrency" / MATRIX_NAME).resolve()
    destination = (args.destination or repo / "docs/performance/backend-concurrency/results" / ARCHIVE_NAME).absolute()
    require(not destination.exists() and not destination.is_symlink(),
            f"Refusing to overwrite existing destination: {destination}")

    # Preflight selects/validates evidence at invocation time, not when this script was written.
    summary = read_json(validation / "regressions/accepted-regression-summary.json")
    suites = summary["suites"]
    require(summary["failed_executions"] == 0, "Final regression summary contains failures")
    require(summary["suite_count"] == len(suites) == args.expected_suites,
            "Unexpected number of final regression suites")
    require({s["index"] for s in suites} == set(range(args.expected_suites)),
            "Missing or duplicate final regression suite indexes")
    for suite in suites:
        require(suite["returncode"] == 0 and suite["counts"][0] == "ok"
                and int(suite["counts"][1]) > 0 and int(suite["counts"][2]) == 0,
                f"Regression did not pass: {suite}")

    metadata = read_json(matrix / "metadata.json")
    require(metadata.get("status") == "completed" and metadata["run"]["code"] == 0,
            "Matrix is not completed successfully")
    require(metadata["options"]["mode"] == "full", "Expected full matrix")
    require(metadata.get("source_changed_during_run") is False, "Matrix source changed/unknown")
    results = metadata["results"]
    require(results["cases"] == 36 and results["invalidLines"] == 0
            and not results["integrityFailures"] and results.get("missing") is False,
            "Matrix raw-result integrity gate did not pass")
    comparison = read_json(validation / "comparison-final.json")
    require(comparison["acceptance"] == "internally_consistent_evidence"
            and not comparison["comparison_violations"], "Comparison integrity gate failed")

    host_receipt = read_json(validation / "host-read-receipt.json")
    require(host_receipt["exit_code"] == 0 and host_receipt["tests_passed"] > 0,
            "Host measurement did not pass")
    host_binary = Path(host_receipt["binary"])
    require(host_binary.is_file(), f"Host binary unavailable for receipt verification: {host_binary}")
    require(sha256(host_binary) == host_receipt["binary_sha256"],
            "Host receipt is stale for the current production binary; rerun/update evidence")
    host_results = read_json(validation / "host-read-results.json")
    require(host_results["proofEqual"] and host_results["deletionObserved"],
            "Host proof/deletion contract failed")
    checks = read_json(validation / "host-checks-final.json")["checks"]
    require(checks and all(check["exit_code"] == 0 for check in checks), "Host checks failed")
    for name in ("refresh-unit-final.json", "refresh-browser-final.json"):
        receipt = read_json(validation / name)
        require(receipt["exit_code"] == 0, f"Failed receipt: {name}")
        require(sha256(validation / Path(receipt["log"]).name) == receipt["log_sha256"],
                f"Receipt/log mismatch: {name}")

    plan = []
    destinations = set()
    optional_missing = []

    def add(source: Path, target: str, *, optional=False, force_gzip=False):
        if optional and not source.exists():
            optional_missing.append(str(source))
            return
        require(source.is_file() and not source.is_symlink(), f"Missing/nonregular evidence: {source}")
        # The allowlist never contains databases, keys or executable binaries.
        require(source.suffix in (".log", ".json", ".jsonl", ".txt", ".md", ".py",
                                  ".ts", ".tsx", ".cjs", ".mjs", ".csv"),
                f"Unexpected evidence type: {source}")
        require(not Path(target).is_absolute() and ".." not in Path(target).parts,
                f"Unsafe destination: {target}")
        compressed = force_gzip or source.suffix == ".log" or (source.suffix in (".jsonl",)
                                     and source.stat().st_size >= COMPRESS_THRESHOLD)
        target += ".gz" if compressed else ""
        require(target not in destinations, f"Duplicate archive path: {target}")
        destinations.add(target)
        plan.append((source, target, compressed, stable_stat(source)))

    for name in ("metadata.json", "resources.jsonl", "cases.jsonl", "test.stdout.log",
                 "test.stderr.log", "build.stdout.log", "build.stderr.log"):
        add(matrix / name, name, optional=name.startswith("build.") and metadata.get("binary", {}).get("source") == "provided", force_gzip=name == "cases.jsonl")
    add(matrix / "summary.json", "summary.json", optional=True)
    for suffix in ("json", "csv", "txt"):
        add(validation / f"comparison-final.{suffix}", f"comparison.{suffix}", force_gzip=suffix == "json")
    add(repo / "docs/performance/backend-concurrency/results/2026-10-11-round4/compare-results.py",
        "compare-results.py")
    for name in ACCEPTED:
        add(validation / name, "accepted/" + name)
    for name in HOST:
        add(validation / name, "measurements/host/" + name)
    for name in RENDERER_INPUTS:
        add(validation / name, "measurements/renderer/" + name)
    for name in REFRESH_INPUTS:
        add(validation / name, "measurements/refresh/" + name)
    for name in BUILD_RECORDS:
        add(validation / name, "build-records/" + name)
    for name in AUDITS:
        add(validation / name, "audits/" + name + (".txt" if name.endswith(".md") else ""))
    add(validation / "regressions/accepted-regression-summary.json",
        "regressions/accepted-regression-summary.json")
    for suite in suites:
        require(Path(suite["log"]).name == suite["log"], "Unsafe regression log path")
        add(validation / "regressions" / suite["log"], "regressions/" + suite["log"])
    # Keep only result batches fully represented by the accepted summary, so stale JSON cannot
    # accidentally inflate final counts. The summary alone is the authoritative aggregate.
    accepted_by_index = {suite["index"]: suite for suite in suites}
    for source in sorted((validation / "regressions").glob("results-*.json")):
        batch = read_json(source)
        if batch and all(item == accepted_by_index.get(item["index"]) for item in batch):
            add(source, "regressions/" + source.name)
    for name in EARLY_CHECKPOINT:
        add(validation / name, "diagnostics/checkpoint-first/" + name, optional=True)
    for name in EARLY_FAILURES:
        add(validation / name, "diagnostics/earlier-failures/" + name, optional=True)
    # Earlier release results are diagnostic, regardless of whether some individual suites passed.
    early = validation / "pre-fixture-fix-regressions"
    failed_logs = set()
    for source in sorted(early.glob("results-*.json")):
        add(source, "diagnostics/pre-fixture-fix-regressions/" + source.name)
        for item in read_json(source):
            if item.get("returncode") or not item.get("counts") or item["counts"][0] != "ok":
                failed_logs.add(item["log"])
    for name in sorted(failed_logs):
        require(Path(name).name == name, "Unsafe earlier log path")
        add(early / name, "diagnostics/pre-fixture-fix-regressions/" + name)
    for name in ("run-regressions.py", "summarize-regressions.py", "make-summary.py", "repeat-recovery-barrier.py"):
        add(validation / name, "scripts/" + name)
    add(Path(__file__).resolve(), "scripts/archive-evidence.py")

    print(json.dumps({"destination": str(destination), "write": args.write,
                      "files": [target for _, target, _, _ in plan],
                      "optional_missing": optional_missing}, indent=2))
    if not args.write:
        print("Preflight only; no files written. After review, pass --write.")
        return

    destination.parent.mkdir(parents=True, exist_ok=True)
    # Reserve the destination with exclusive mkdir. Directory rename on POSIX can replace an
    # empty directory created by somebody else between an exists check and rename, so do not
    # publish through rename. SHA256SUMS is written last and indicates a completed archive.
    stage = destination
    stage.mkdir(exist_ok=False)
    manifest = []
    try:
        for source, relative, compressed, expected_stat in plan:
            require(stable_stat(source) == expected_stat, f"Evidence changed after preflight: {source}")
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            source_hash = sha256(source)
            with source.open("rb") as origin, target.open("xb") as output:
                if compressed:
                    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as zipper:
                        shutil.copyfileobj(origin, zipper, length=1024 * 1024)
                else:
                    shutil.copyfileobj(origin, output, length=1024 * 1024)
            require(stable_stat(source) == expected_stat and sha256(source) == source_hash,
                    f"Evidence changed while copying: {source}")
            manifest.append({"source": str(source.relative_to(repo)) if source.is_relative_to(repo) else str(source),
                             "archive": relative, "source_sha256": source_hash,
                             "archive_sha256": sha256(target), "gzip": compressed})

        (stage / "archive-manifest.json").write_text(json.dumps({
            "created_at_utc": datetime.now(timezone.utc).isoformat(),
            "matrix_source": str(matrix), "validation_source": str(validation),
            "host_binary_sha256": host_receipt["binary_sha256"],
            "files": manifest, "optional_missing": optional_missing,
            "note": "Explicit evidence selection. Diagnostics are excluded from final counts. No SLO claim.",
        }, indent=2) + "\n", encoding="utf-8")
        readme = f"""第五轮证据归档；最终结论以本轮结果页为准。

根目录为 full matrix 原始 metadata/cases/resources、stdout/stderr 和 comparison。
accepted/ 为明确选定的 TS/browser/Host 检查、Clippy、格式与基础设施门禁。
regressions/accepted-regression-summary.json 是唯一最终测试计数入口。
build-records/ 保留不同构建阶段记录；测试与生产二进制可能不同，不能混用其 SHA。
measurements/host/ 是真实隔离 Electron 管道窄读取；各 20 RPC，非饱和/paint/SLO 证据。
measurements/renderer/ 是 advisory Node/Vite 序列化样本，不能作隔离 CPU 门禁。
measurements/refresh/ 含受控浏览器结果日志、数据、配置与 baseline；不复制截图，源截图保持原样。
diagnostics/checkpoint-first/ 单列早期 checkpoint 门禁；其余早期失败也仅供排错。
diagnostics/ 中的源码阶段、二进制、失败和通过数不得混入最终回归或性能样本。
audits/ 保存工作说明，可能描述早期阶段；以最终摘要和原始日志交叉核对。

复算比较（仓库根目录；输出必须是未使用的新前缀）：
python3 docs/performance/backend-concurrency/results/{ARCHIVE_NAME}/compare-results.py docs/performance/backend-concurrency/results/{ARCHIVE_NAME} --baseline docs/performance/backend-concurrency/results/2026-10-11-round4 --output-prefix .cache/backend-concurrency/round5-recomparison-new
python3 docs/performance/backend-concurrency/results/{ARCHIVE_NAME}/compare-results.py --self-test

scripts/ 保留本轮执行脚本。run-regressions.py/summarize-regressions.py 必须先复制到
新的隔离验证目录，核对其中 binary/target 路径和最终 SHA 后才可运行，勿写归档目录。
archive-evidence.py 默认只 preflight；审阅选定日志后用 --write 显式创建全新输出目录。
它运行时读取最新证据，核对 Host binary receipt，拒绝已有目标路径，不删除任何文件。
在缺少最终 matrix/comparison/accepted summary 时会拒绝生成不完整的最终归档。

全部原始 log，以及 >= {COMPRESS_THRESHOLD} 字节的 jsonl 和 cases/comparison JSON 使用 gzip mtime=0；
压缩前后路径及 SHA 见 archive-manifest.json。SHA256SUMS 覆盖自身之外全部文件。
没有数据库、密钥或可执行二进制归档。内部一致性通过不代表整体性能或 SLO 达标。
"""
        (stage / "README.txt").write_text(readme, encoding="utf-8")
        sums = "".join(f"{sha256(path)}  {path.relative_to(stage).as_posix()}\n"
                       for path in sorted(stage.rglob("*")) if path.is_file())
        (stage / "SHA256SUMS").write_text(sums, encoding="utf-8")
    except BaseException:
        print(f"Incomplete new archive retained for inspection: {stage}", file=sys.stderr)
        raise
    print(f"Archive published: {destination}")


if __name__ == "__main__":
    main()
