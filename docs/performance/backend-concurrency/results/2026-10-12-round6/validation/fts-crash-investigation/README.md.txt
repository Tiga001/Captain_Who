# Round6 FTS integrity diagnostic — isolated synthetic fixtures only

Date: 2026-10-11. This record concerns the first fault-test run, not performance measurements.

## Observed failure and bounded conclusion

`../storage-faults.log` records 3 passed / 1 failed. FULL rollback and SQLite Backup API business-restore tests passed. The terminal crash test failed when `assert_terminal_state` ran `PRAGMA quick_check` a second time on a reused pooled read connection after the test had committed a terminal retry through another connection. The diagnostic was `fts5: checksum mismatch for table "conversation_history_fts"`.

A fresh read-only handle sees `quick_check = ok` in both before-commit and after-commit killed-process databases. Original content and FTS content agree. A separate minimal ordinary-table + FTS-trigger fixture reproduces the same misleading integrity result without any process crash or application schema. The exact SQL sequence also reproduces with SQLite 3.51.0, whose wording is `malformed inverted index for FTS5 table main.evidence`. This is not evidence of a newly introduced 3.53.2 regression.

The observable conclusion is a connection-state-sensitive FTS integrity diagnostic. A stale per-connection FTS index/check structure is the working explanation, supported by fresh-connection checks and an ordinary MATCH read clearing the symptom; the internal root cause has not been confirmed upstream. These results do not prove absence of every possible storage defect, and do not establish behavior for all SQLite versions, FTS options, or transaction patterns.

## Preserved minimal reproduction

`minimal.c` creates a new WAL database with a messages table, an ordinary FTS5 trigram table, and INSERT/UPDATE triggers. It refuses an already-existing target. It runs:

1. Insert `confirmed checkpoint`; read-only reader quick_check is ok.
2. Separate writer updates ordinary message to `committed terminal`; its trigger deletes/reinserts FTS.
3. Original reader SELECTs the ordinary table and sees committed terminal.
4. Same reader quick_check reports checksum mismatch.
5. A newly opened read-only handle quick_check is ok.
6. Original reader MATCH 'committed' returns committed terminal.
7. Original reader quick_check is now ok.

No step repairs, rebuilds, or writes through the original reader. The writer's only data operation after initial seed is the ordinary UPDATE in step 2. Minimal output is saved as `minimal-3.53.2.txt`.

Commands run from repository root (existing SQLite archive was reused; no parallel Cargo build):

```sh
clang -O0 .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/minimal.c <build>/release/build/libsqlite3-sys-1f76c2f51daa99b1/out/libsqlite3.a -o .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/minimal
.cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/minimal .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/minimal-source.sqlite > .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/minimal-3.53.2.txt
python3 .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/reproduce_python.py > .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/minimal-python-3.51.0.txt
```

For a repeat C run, pass a new filename in a new `mktemp -d` directory because the reproducer rejects existing files. Its runtime output identifies the linked SQLite version. The installed Python runtime used here reported 3.51.0; `reproduce_python.py` creates a TemporaryDirectory and prints the actual version, so it is reproducible without retained database artifacts but may report a different version on a different host. The C compiler emitted an SDK deployment-version warning (archive macOS 26.5, local link target 26.0); both diagnostics executed successfully. Source/header ABI entry points are basic SQLite C APIs.

## Application crash fixtures and original-index checks

The actual test fixture is `storage::service::tests::storage_faults::terminal_crash_fixture` in `crates/core/src/storage/service/tests/storage_faults.rs`. The reproduction binary was `<build>/release/deps/mycopilot_core-7748a31bd053ad1f`. The companion `recreate_crash_fixtures.py` documents/recreates the exact ready-marker + child-only kill procedure. It requires the test binary as argument; it only kills children that it starts and creates new synthetic directories.

Original synthetic samples retained for this investigation:

- `before-commit-yl30y9um/crash.sqlite` with associated WAL/SHM: confirmed checkpoint prefix, no terminal facts.
- `after-commit-rhpoivh3/crash.sqlite` with associated WAL/SHM: completed terminal and narration trace.

The source samples were read only for the original-index probes. Separate copies named `integrity-copy.sqlite` were used for the explicit FTS5 `integrity-check` control via Python SQLite 3.51.0; both passed. Explicit FTS checks are not part of the modified production/fault-test path and were never run on user data.

`probe.c` and `fresh-before.txt` / `fresh-after.txt` preserve the linked SQLite 3.53.2 fresh-handle quick_check and raw FTS content evidence. Compile/run commands:

```sh
clang -O0 .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/probe.c <build>/release/build/libsqlite3-sys-1f76c2f51daa99b1/out/libsqlite3.a -o .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/probe
.cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/probe .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/before-commit-yl30y9um/crash.sqlite > .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/fresh-before.txt
.cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/probe .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/after-commit-rhpoivh3/crash.sqlite > .cache/backend-concurrency/round6-validation-20261011/fts-crash-investigation/fresh-after.txt
```

Passing any third argument to probe enables its ordinary UPDATE diagnostic. This was done ONLY on a new copy of the before-commit sample (`cache-probe-second.sqlite`, with copied WAL/SHM), never the original sample. `stale-reader-reproduction.txt` records old-reader mismatch -> correct MATCH -> old-reader ok on the application schema. The smaller `minimal.c` is the preferred repeatable reproducer.

## Test correction and verification status

Only `assert_terminal_state` changed: release pooled reader after checking persisted message status/content/checkpoint; open a fresh READ_ONLY connection to that same temporary database; run quick_check and foreign_key_check there; retain original Trace/model-context/usage/notification assertions. No production SQLite settings, pool behavior, canonical FTS triggers, commit/rollback logic, source data, or FTS index were repaired or weakened. The test still verifies the actual original durable database after each controlled crash/retry.

The initial failing test was rerun once through the old binary with RUST_BACKTRACE=1 and reproduced. After the correction, unified build/test is owned by the parent agent; this record does not yet claim that the recompiled fault filter passed. Target filter:

`storage::service::tests::storage_faults::process_kill_at_terminal_commit_boundary_restores_an_atomic_prefix`

No performance benchmark ran in this investigation. The original crash samples and diagnostics are synthetic and can be removed after archival; retain these sources, command record, text outputs and the fault test so the evidence can be recreated.
