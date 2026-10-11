"""Recreate synthetic WAL crash samples; requires an already compiled core test binary."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

fixture = "storage::service::tests::storage_faults::terminal_crash_fixture"
base = Path(__file__).resolve().parent
if len(sys.argv) != 2:
    raise SystemExit("usage: python3 recreate_crash_fixtures.py /absolute/path/to/core-test-binary")
for phase in ("before-commit", "after-commit"):
    root = Path(tempfile.mkdtemp(prefix=phase + "-", dir=base))
    env = os.environ.copy()
    env["CAPTAIN_STORAGE_TERMINAL_CRASH_FIXTURE"] = str(root)
    env["CAPTAIN_STORAGE_TERMINAL_CRASH_PHASE"] = phase
    with (root / "fixture.log").open("w") as log:
        child = subprocess.Popen([sys.argv[1], "--exact", fixture, "--nocapture"], env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 30
            while not (root / "ready").exists():
                if child.poll() is not None:
                    raise RuntimeError(f"fixture exited before ready; see {root}")
                if time.monotonic() > deadline:
                    raise RuntimeError(f"fixture timed out; see {root}")
                time.sleep(0.01)
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)
    print(root / "crash.sqlite")
