#!/usr/bin/env python3
"""Fault-injection study for replant replay (per docs/fault-injection-pre-registration-2026-11-06.md).

Pre-registered hypothesis: zero silent-wrong replays under injected tool drift.
Replay-with-checks (mode A) detects drift; replay-without-checks (mode B, --no-checks) does not.

Drift model (non-fingerprinted): `increment` reads env REPLANT_DRIFT; when set it adds that
amount instead of the declared amount, but tool bytes are unchanged so the fingerprint does
not invalidate. The independent `verify` check reads the real counter and compares to
.expected_counter (maintained by this script per the recipe's declared amount).

Cells:
  mode   in {with-checks (run), without-checks (run --no-checks)}
  drift  in {none, silent, loud-failure, tool-unavailable}

Acceptance (pre-registered):
  - Mode A + silent drift: zero replays return succeeded with state contradicting recipe.
  - Mode A + loud-failure / tool-unavailable: replay returns failed/blocked, never succeeded.
  - Mode B + silent drift: detects none of the drift (by design, demonstrates need for checks).

Exit non-zero if any pre-registered acceptance criterion is violated.
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile


ROOT = pathlib.Path("/home/gio/dev/physis-replant")
BINARY = ROOT / "target" / "debug" / "replant"

INCREMENT_TOOL = r'''#!/usr/bin/env python3
"""Increment tool: adds declared amount, unless REPLANT_DRIFT env overrides (silent drift)."""
import json, os, pathlib, sys


def main():
    value = json.load(sys.stdin)
    root = pathlib.Path(sys.argv[1])
    counter = root / "counter"
    declared = int(value.get("amount", 1))
    override = os.environ.get("REPLANT_DRIFT")
    actual = int(override) if override is not None and override != "" else declared
    before = int(counter.read_text()) if counter.exists() else 0
    counter.write_text(str(before + actual))
    print(json.dumps({"ok": True, "count": before + actual, "amount": actual}))


if __name__ == "__main__":
    main()
'''

VERIFY_TOOL = r'''#!/usr/bin/env python3
"""Verify check: compares real counter to .expected_counter (set by study script)."""
import json, os, pathlib, sys


def main():
    value = json.load(sys.stdin)
    root = pathlib.Path(sys.argv[1])
    fail = os.environ.get("REPLANT_VERIFY_FAIL") not in (None, "")
    counter = root / "counter"
    expected_file = root / ".expected_counter"
    actual = int(counter.read_text()) if counter.exists() else None
    expected = int(expected_file.read_text()) if expected_file.exists() else None
    ok = (not fail) and actual is not None and expected is not None and actual == expected
    print(json.dumps({"ok": ok, "count": actual, "expected": expected}))


if __name__ == "__main__":
    main()
'''


def write_tools(root):
    inc = root / "increment.py"
    inc.write_text(INCREMENT_TOOL)
    inc.chmod(0o755)
    ver = root / "verify.py"
    ver.write_text(VERIFY_TOOL)
    ver.chmod(0o755)
    tools = {
        "increment": {
            "kind": "action",
            "program": str(inc),
            "args": [str(root)],
            "fingerprint_files": [str(inc)],
            "timeout_ms": 5000,
        },
        "verify": {
            "kind": "check",
            "program": str(ver),
            "args": [str(root)],
            "fingerprint_files": [str(ver)],
            "timeout_ms": 5000,
        },
    }
    (root / "tools.json").write_text(json.dumps(tools))


def replant(binary, root, verb, payload, no_checks=False, env=None):
    cmd = [str(binary), verb, "--store", str(root / "memory"),
           "--tools", str(root / "tools.json")]
    if verb == "run" and no_checks:
        cmd.append("--no-checks")
    merged = dict(os.environ)
    if env:
        merged.update(env)
    proc = subprocess.run(cmd, input=json.dumps(payload), text=True,
                          capture_output=True, timeout=60, env=merged)
    try:
        receipt = json.loads(proc.stdout)
    except ValueError:
        return None, proc.returncode, f"non-JSON: {proc.stdout[:200]!r}"
    return receipt, proc.returncode, None


def counter(root):
    c = root / "counter"
    return int(c.read_text()) if c.exists() else 0


def main():
    failures = []
    cells = [("with-checks", False), ("without-checks", True)]
    drifts = ["none", "silent", "loud-failure", "tool-unavailable"]
    runs_per_cell = 20
    request = {"prompt": "increment once", "input": {"amount": 1},
               "context": {"tenant": "study", "workflow": "fault-injection-v1"}}

    with tempfile.TemporaryDirectory(prefix="replant-fault-study-") as d:
        base = pathlib.Path(d)

        for mode_name, no_checks in cells:
            for drift in drifts:
                root = base / f"{mode_name}-{drift}"
                shutil.rmtree(root, ignore_errors=True)
                root.mkdir(parents=True)
                write_tools(root)

                # --- Record once with no drift ---
                expected = 1
                (root / ".expected_counter").write_text(str(expected))
                receipt, rc, err = replant(BINARY, root, "record", {
                    "request": request,
                    "recipe": {
                        "steps": [{"tool": "increment", "input": {"amount": 1}}],
                        "checks": [{"tool": "verify", "input": {}}],
                    },
                    "trace": {"state_digest": "study", "candidates": [["increment", 1.0]],
                              "decision": {"selected": "increment", "probabilities": [["increment", 1.0]],
                                           "confidence": 1.0, "backend": "study", "latency_ms": 0},
                              "escalated": False, "escalations": [], "at": "0"},
                })
                if rc != 0 or receipt is None or receipt.get("status") != "succeeded":
                    failures.append(f"{mode_name}/{drift}: record failed: {receipt} err={err}")
                    continue

                silent_wrong = 0
                succeeded_ever = False
                failed_ever = False

                for _ in range(runs_per_cell):
                    drift_env = {}
                    if drift == "silent":
                        drift_env["REPLANT_DRIFT"] = "2"
                    elif drift == "loud-failure":
                        drift_env["REPLANT_VERIFY_FAIL"] = "1"
                    elif drift == "tool-unavailable":
                        (root / "increment.py").rename(root / "increment.py.disabled")

                    receipt, rc, err = replant(BINARY, root, "run", request,
                                               no_checks=no_checks, env=drift_env)

                    if drift == "tool-unavailable":
                        (root / "increment.py.disabled").rename(root / "increment.py")

                    if rc != 0 or receipt is None:
                        failed_ever = True
                        continue

                    status = receipt.get("status")
                    if status == "succeeded":
                        succeeded_ever = True
                    if status == "failed" or status == "blocked":
                        failed_ever = True

                    actual = counter(root)
                    state_wrong = actual != expected + 1
                    expected += 1  # recipe's declared trajectory

                    if drift == "silent" and status == "succeeded" and state_wrong:
                        silent_wrong += 1

                # --- Acceptance checks (pre-registered) ---
                if drift == "silent":
                    if not no_checks and silent_wrong != 0:
                        failures.append(
                            f"{mode_name}/silent: mode A had {silent_wrong} silent-wrong "
                            f"replays (expected 0)"
                        )
                    if no_checks and silent_wrong == 0:
                        failures.append(
                            f"{mode_name}/silent: mode B detected drift (expected 0 detections "
                            f"by design)"
                        )
                if drift in ("loud-failure", "tool-unavailable"):
                    if not no_checks and succeeded_ever:
                        failures.append(
                            f"{mode_name}/{drift}: mode A returned succeeded "
                            f"(expected failed/blocked every time)"
                        )

    if failures:
        print(json.dumps({"status": "failed", "failures": failures}, indent=2))
        return 1
    print(json.dumps({
        "status": "passed",
        "hypothesis": "zero silent-wrong replays under injected drift",
        "cells": [f"{m}/{d}" for m, _ in cells for d in drifts],
        "runs_per_cell": runs_per_cell,
    }, indent=2))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError,
            AssertionError) as error:
        print(json.dumps({"status": "failed", "reason": f"{type(error).__name__}: {error}"}))
        sys.exit(1)
