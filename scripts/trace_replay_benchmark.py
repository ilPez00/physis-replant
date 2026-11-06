#!/usr/bin/env python3
"""Measure replay hit rate on a repeated-request trace, using real CLI processes.

This is NOT a live model benchmark: the planner is a local fixture and no
inference is called. It measures how often identical requests are served by
replay (real actions + independent checks, zero provider calls) versus planned
from scratch, and states a zero-token hand-written-script baseline for context.

Usage:
  cargo build --offline --bin replant
  python3 scripts/trace_replay_benchmark.py \
      --trace examples/traces/counter_repeats.jsonl --passes 3

Exit non-zero on any failed execution or unverified side effect.
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
TOKENS_PER_PLAN = 452  # measured n8n/gemma4-e2b planning turn (prompt+completion)


def local_planner(request):
    """Fixture planner: one increment step + one independent verify check."""
    step_tool = "decrement" if request["prompt"].startswith("decrement") else "increment"
    return {
        "request": request,
        "recipe": {
            "steps": [{"tool": step_tool, "input": request.get("input", {})}],
            "checks": [{"tool": "verify", "input": {}}],
        },
        "trace": {
            "state_digest": hashlib.sha256(
                json.dumps(request, sort_keys=True).encode()
            ).hexdigest(),
            "candidates": [[step_tool, 1.0]],
            "decision": {
                "selected": step_tool,
                "probabilities": [[step_tool, 1.0]],
                "confidence": 1.0,
                "backend": "fixture:local-planner-not-llm",
                "latency_ms": 0,
            },
            "escalated": False,
            "escalations": [],
            "at": str(time.time_ns()),
        },
    }


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
        raise RuntimeError(f"{verb} failed (exit {output.returncode}): {receipt}")
    if receipt.get("status") in ("failed", "blocked"):
        raise RuntimeError(f"{verb} returned terminal status: {receipt}")
    return receipt, {"command": verb, "status": receipt["status"],
                     "operation": receipt.get("operation"), "wall_ms": elapsed_ms}


def counter_value(root):
    counter = root / "counter"
    return int(counter.read_text()) if counter.exists() else 0


def check_count(root):
    checks = root / "checks.jsonl"
    if not checks.exists():
        return 0
    return sum(1 for line in checks.read_text().splitlines() if line.strip())


def last_check_ok(root):
    checks = root / "checks.jsonl"
    if not checks.exists():
        return False
    lines = [line for line in checks.read_text().splitlines() if line.strip()]
    if not lines:
        return False
    return json.loads(lines[-1]).get("ok") is True


def setup_root(root):
    root.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "examples" / "fixture_tools.py", root / "tools.py")
    tools = {
        name: {
            "kind": kind, "program": str(pathlib.Path(sys.executable).resolve()),
            "args": [str(root / "tools.py"), mode, str(root)],
            "fingerprint_files": [str(root / "tools.py")], "timeout_ms": 2000,
        }
        for name, kind, mode in [("increment", "action", "increment"),
                                 ("decrement", "action", "decrement"),
                                 ("verify", "check", "verify")]
    }
    (root / "tools.json").write_text(json.dumps(tools))


def run_request(binary, root, request, planner_calls):
    """Run one request through `run`, recording on miss. Returns (kind, timing)."""
    before_checks = check_count(root)
    before_counter = counter_value(root)
    receipt, timing = call(binary, root, "run", request)
    status = receipt["status"]
    if status == "succeeded":
        # Independent side-effect verification: each execution appends an ok check
        # and advances the counter. Receipt alone is not trusted.
        assert check_count(root) > before_checks, "no new independent check recorded"
        assert last_check_ok(root), "independent check did not report ok"
        assert counter_value(root) == before_counter + int(request["input"].get("amount", 1)), \
            "counter did not advance by expected amount"
        return "replay", timing, planner_calls
    if status == "miss":
        planner_calls += 1
        record = local_planner(request)
        receipt, timing = call(binary, root, "record", record)
        if receipt["status"] != "succeeded":
            raise RuntimeError(f"record did not succeed: {receipt}")
        assert check_count(root) > before_checks and last_check_ok(root), "check missing after record"
        assert counter_value(root) == before_counter + int(request["input"].get("amount", 1)), \
            "counter did not advance by expected amount"
        return "miss", timing, planner_calls
    if status == "review":
        # Near-match: nothing executed, side effects unchanged.
        assert check_count(root) == before_checks, "review executed effects"
        assert counter_value(root) == before_counter, "review changed state"
        return "review", timing, planner_calls
    raise RuntimeError(f"unexpected run status {status}: {receipt}")


def trace_requests(path):
    requests = []
    with path.open() as stream:
        for line in stream:
            line = line.strip()
            if line:
                requests.append(json.loads(line))
    if not requests:
        raise RuntimeError("trace is empty")
    return requests


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path,
                        default=ROOT / "target" / "debug" / "replant")
    parser.add_argument("--trace", type=pathlib.Path, required=True,
                        help="JSONL file of {prompt,input,context} requests")
    parser.add_argument("--passes", type=int, default=3,
                        help="steady-state measured passes over the trace")
    parser.add_argument("--tokens-per-plan", type=int, default=TOKENS_PER_PLAN)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"binary not found: {binary}; build with cargo build --offline --bin replant")
    if args.passes < 1:
        parser.error("passes must be at least 1")

    requests = trace_requests(args.trace)

    with tempfile.TemporaryDirectory(prefix="replant-trace-benchmark-") as directory:
        root = pathlib.Path(directory)

        # ---- Phase A: cold learning (one-time cost). Fresh store, every request
        # misses and is planned + recorded once. Measures learning, not steady state.
        setup_root(root / "cold")
        cold = {"planner_calls": 0, "hits": 0, "reviews": 0, "misses": 0,
                "per_pass": []}
        for request in requests:
            kind, timing, cold["planner_calls"] = run_request(
                binary, root / "cold", request, cold["planner_calls"])
            if kind == "replay":
                cold["hits"] += 1
            elif kind == "miss":
                cold["misses"] += 1
            else:
                cold["reviews"] += 1
        cold_total_tokens = cold["planner_calls"] * args.tokens_per_plan

        # ---- Phase B: steady state. Fresh store pre-seeded with verified recipes
        # for the exact/base requests; near-match variants left unrecorded so they
        # surface as `review` (require explicit approval) rather than silent replay.
        setup_root(root / "steady")
        # Pre-seed steady store with verified recipes for the exact base requests
        # (near-match and different requests are deliberately not recorded).
        for request in requests:
            kind, timing, _ = run_request(binary, root / "steady", request, 0)
            if kind not in ("replay", "miss", "review"):
                raise RuntimeError(f"pre-seed unexpected kind {kind}: {request}")

        steady_passes = []
        for _ in range(args.passes):
            ps = {"planner_calls": 0, "hits": 0, "reviews": 0, "misses": 0}
            for request in requests:
                kind, timing, ps["planner_calls"] = run_request(
                    binary, root / "steady", request, ps["planner_calls"])
                if kind == "replay":
                    ps["hits"] += 1
                elif kind == "miss":
                    ps["misses"] += 1
                else:
                    ps["reviews"] += 1
            total = len(requests)
            ps["hit_rate"] = ps["hits"] / total
            ps["review_rate"] = ps["reviews"] / total
            ps["miss_rate"] = ps["misses"] / total
            steady_passes.append(ps)

        steady_planner_calls = sum(p["planner_calls"] for p in steady_passes)
        steady_hits = sum(p["hits"] for p in steady_passes)
        steady_reviews = sum(p["reviews"] for p in steady_passes)
        steady_total = args.passes * len(requests)
        steady_hit_rate = steady_hits / steady_total
        steady_review_rate = steady_reviews / steady_total

        plan_every_time_tokens = steady_total * args.tokens_per_plan
        replant_tokens = steady_planner_calls * args.tokens_per_plan
        saved_percent = (1 - replant_tokens / plan_every_time_tokens) * 100 if plan_every_time_tokens else 0.0

        report = {
            "scope": "real CLI subprocesses; local fixture planner, NO live model or inference; "
                     "every replay and record re-executes real actions + independent checks",
            "trace": str(args.trace), "requests_per_pass": len(requests),
            "passes": args.passes, "tokens_per_plan": args.tokens_per_plan,
            "phase_a_cold_learning": {
                "planner_calls": cold["planner_calls"],
                "hits": cold["hits"], "reviews": cold["reviews"], "misses": cold["misses"],
                "tokens": cold_total_tokens,
                "note": "one-time cost to learn verified recipes; not steady state",
            },
            "phase_b_steady_state": {
                "planner_calls": steady_planner_calls,
                "hits": steady_hits, "reviews": steady_reviews, "misses": sum(p["misses"] for p in steady_passes),
                "hit_rate": round(steady_hit_rate, 4),
                "review_rate": round(steady_review_rate, 4),
                "miss_rate": round(sum(p["misses"] for p in steady_passes) / steady_total, 4),
                "per_pass": steady_passes,
            },
            "tokens": {
                "plan_every_time": plan_every_time_tokens,
                "replant_steady_state": replant_tokens,
                "saved_percent": round(saved_percent, 2),
                "hand_written_script_zero_tokens": 0,
                "note": "replant beats repeated planning only when the recipe must come from an agent; "
                        "a human-written fixed script needs zero tokens and is the true zero-cost baseline",
            },
            "binary": str(binary),
        }
        print(json.dumps(report, indent=2))
        (pathlib.Path(directory) / "report.json").write_text(json.dumps(report, indent=2))
        return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError,
            AssertionError) as error:
        print(json.dumps({"status": "failed", "reason": f"{type(error).__name__}: {error}"}))
        sys.exit(1)
