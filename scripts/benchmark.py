#!/usr/bin/env python3
"""Measure real CLI processes with a local planner fixture, NOT a live model.

This is an n8n-compatible process loop, NOT a live n8n runtime benchmark.
Build first: cargo build --offline --bin replant
"""
import argparse
import hashlib
import json
import pathlib
import shutil
import statistics
import subprocess
import sys
import tempfile
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]


def plan(request):
    """A deliberately trivial local planner; no inference or simulated delay."""
    started = time.perf_counter()
    record = {
        "request": request,
        "recipe": {
            "steps": [{"tool": "increment", "input": request["input"]}],
            "checks": [{"tool": "verify", "input": {}}],
        },
        "trace": {
            "state_digest": hashlib.sha256(
                json.dumps(request, sort_keys=True).encode()
            ).hexdigest(),
            "candidates": [["increment", 1.0]],
            "decision": {
                "selected": "increment",
                "probabilities": [["increment", 1.0]],
                "confidence": 1.0,
                "backend": "fixture:local-planner-not-llm",
                "latency_ms": 0,
            },
            "escalated": False,
            "escalations": [],
            "at": str(time.time_ns()),
        },
    }
    elapsed_ms = (time.perf_counter() - started) * 1000
    record["trace"]["decision"]["latency_ms"] = int(elapsed_ms)
    return record, elapsed_ms


def call(binary, root, verb, payload):
    started = time.perf_counter()
    output = subprocess.run(
        [str(binary), verb, "--store", str(root / "memory"),
         "--tools", str(root / "tools.json")],
        input=json.dumps(payload), text=True, capture_output=True, timeout=30,
        check=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    try:
        receipt = json.loads(output.stdout)
    except ValueError as error:
        raise RuntimeError(
            f"{verb} did not emit JSON: stdout={output.stdout[:500]!r}; "
            f"stderr={output.stderr[:500]!r}"
        ) from error
    if output.returncode != 0:
        raise RuntimeError(f"{verb} failed: {receipt}")
    if receipt.get("status") not in ("miss", "succeeded"):
        raise RuntimeError(f"unexpected {verb} receipt: {receipt}")
    if receipt.get("provider_calls") != 0:
        raise RuntimeError("local fixture must not call a provider")
    return receipt, {
        "command": verb,
        "status": receipt["status"],
        "operation": receipt["operation"],
        "wall_ms": elapsed_ms,
        "engine_elapsed_us": receipt["elapsed_us"],
    }


def run_arm(binary, root, iterations, replay):
    root.mkdir()
    tool_script = root / "tools.py"
    shutil.copyfile(ROOT / "examples" / "fixture_tools.py", tool_script)
    tools = {}
    for name, kind, mode in [("increment", "action", "increment"),
                             ("verify", "check", "verify")]:
        tools[name] = {
            "kind": kind,
            "program": str(pathlib.Path(sys.executable).resolve()),
            "args": [str(tool_script), mode, str(root)],
            "fingerprint_files": [str(tool_script)],
            "timeout_ms": 2000,
        }
    (root / "tools.json").write_text(json.dumps(tools))
    request = {
        "prompt": "increment once",
        "input": {"amount": 1},
        "context": {"tenant": "local-benchmark", "workflow": "counter-v1"},
    }
    samples = []
    planner_calls = 0
    replay_hits = 0
    for expected in range(1, iterations + 1):
        started = time.perf_counter()
        commands = []
        planner_ms = None
        if replay:
            receipt, timing = call(binary, root, "run", request)
            commands.append(timing)
        if not replay or receipt["status"] == "miss":
            planner_calls += 1
            record, planner_ms = plan(request)
            receipt, timing = call(binary, root, "record", record)
            commands.append(timing)
        else:
            if receipt["operation"] != "replay":
                raise RuntimeError("run succeeded without a replay receipt")
            replay_hits += 1
        if receipt["status"] != "succeeded":
            raise RuntimeError(f"execution was not verified: {receipt}")
        execution_ms = (time.perf_counter() - started) * 1000
        # Independent observations, not the receipt's self-reported success.
        count = int((root / "counter").read_text())
        checks = [json.loads(line) for line in
                  (root / "checks.jsonl").read_text().splitlines()]
        if count != expected or len(checks) != expected:
            raise RuntimeError(f"side effects differ at iteration {expected}")
        if checks[-1] != {"ok": True, "count": expected}:
            raise RuntimeError(f"check failed at iteration {expected}")
        samples.append({
            "iteration": expected,
            "execution_wall_ms": execution_ms,
            "verification_wall_ms": (time.perf_counter() - started) * 1000 - execution_ms,
            "planner_wall_ms": planner_ms,
            "commands": commands,
            "observed_counter": count,
            "observed_check": checks[-1],
        })
    expected_plans = 1 if replay else iterations
    if planner_calls != expected_plans:
        raise RuntimeError(f"planner invocations {planner_calls}, expected {expected_plans}")
    timings = [sample["execution_wall_ms"] for sample in samples]
    return {
        "iterations": iterations,
        "planner_invocations": planner_calls,
        "replay_hits": replay_hits,
        "verified_iterations": len(samples),
        "observed_action_count": count,
        "observed_check_count": len(checks),
        "execution_wall_ms": {
            "total": sum(timings), "mean": statistics.mean(timings),
            "median": statistics.median(timings), "min": min(timings), "max": max(timings),
        },
        "samples": samples,
    }


def positive_int(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path,
                        default=ROOT / "target" / "debug" / "replant",
                        help="existing replant executable; no build/install is performed")
    parser.add_argument("--iterations", type=positive_int, default=10)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"binary not found: {binary}; build with cargo build --offline --bin replant")
    try:
        with tempfile.TemporaryDirectory(prefix="replant-benchmark-") as directory:
            root = pathlib.Path(directory)
            baseline = run_arm(binary, root / "baseline", args.iterations, False)
            replay = run_arm(binary, root / "replay", args.iterations, True)
        print(json.dumps({
            "scope": "real CLI subprocesses; local planner fixture NOT a live model; "
                     "n8n-compatible process loop NOT a live n8n runtime benchmark",
            "timing": "perf_counter wall time; execution includes CLI startup, hashing, "
                      "persistent storage, Python tool processes and local planning when used; "
                      "independent verification measured separately; no asserted speedup",
            "binary": str(binary),
            "baseline": baseline,
            "replay": replay,
            "observed_planner_invocations_avoided": (
                baseline["planner_invocations"] - replay["planner_invocations"]
            ),
        }, indent=2))
        return 0
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError) as error:
        print(json.dumps({"status": "failed", "reason": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
