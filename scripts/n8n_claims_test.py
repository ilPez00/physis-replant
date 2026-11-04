#!/usr/bin/env python3
"""Exercise physis-replant's claims through live n8n webhooks.

Requires the three workflows from examples/n8n/ exported and active:
  /webhook/replant-claims         loop: run, record on miss
  /webhook/replant-claims-run     run only
  /webhook/replant-claims-record  record only

Environment overrides:
  REPLANT_N8N_WEBHOOK   default http://127.0.0.1:5678/webhook
  REPLANT_COUNTER       default /home/gio/.replant-n8n/counter/counter
  REPLANT_TOOLS         default /home/gio/.replant-n8n/tools.py
  REPLANT_STORE         default /home/gio/.replant-n8n/state
  REPLANT_BIN           default target/release/replant relative to repo root
"""
import json
import os
import pathlib
import subprocess
import time
import urllib.request

WEBHOOK = os.environ.get("REPLANT_N8N_WEBHOOK", "http://127.0.0.1:5678/webhook")
LOOP = WEBHOOK + "/replant-claims"
RUN = WEBHOOK + "/replant-claims-run"
RECORD = WEBHOOK + "/replant-claims-record"
COUNTER = pathlib.Path(os.environ.get("REPLANT_COUNTER", "/home/gio/.replant-n8n/counter/counter"))
TOOLS = pathlib.Path(os.environ.get("REPLANT_TOOLS", "/home/gio/.replant-n8n/tools.py"))
STORE = os.environ.get("REPLANT_STORE", "/home/gio/.replant-n8n/state")
ROOT = pathlib.Path(__file__).resolve().parents[1]
BIN = os.environ.get("REPLANT_BIN", str(ROOT / "target" / "release" / "replant"))


def post(url, payload):
    body = json.dumps(payload).encode()
    req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as response:
        return json.loads(response.read())


def request_for(context, amount=1):
    return {"prompt": "increment in n8n", "input": {"amount": amount}, "context": {"workflow": context}}


def record_for(request):
    return {
        "request": request,
        "recipe": {
            "steps": [{"tool": "increment", "input": {"amount": 1}}],
            "checks": [{"tool": "verify", "input": {}}],
        },
        "trace": {"provider": "claims-test-external-planner"},
    }


def counter():
    return int(COUNTER.read_text())


def stats():
    out = subprocess.check_output([BIN, "stats", "--store", STORE], text=True)
    return json.loads(out)


def main():
    results = []
    stamp = str(int(time.time()))

    # A. Five identical calls through the loop: one record, four replays, five actions.
    context = "loop-" + stamp
    req = request_for(context)
    rec = record_for(req)
    start_counter = counter()
    operations = []
    for _ in range(5):
        receipt = post(LOOP, {"request": req, "record": rec})
        assert receipt["status"] == "succeeded", receipt
        operations.append(receipt["operation"])
        assert receipt["provider_calls"] == 0
    assert operations == ["record"] + ["replay"] * 4, operations
    assert counter() == start_counter + 5, (start_counter, counter())
    results.append({"claim": "one planner call, four replays, five real actions",
                    "operations": operations, "counter_delta": counter() - start_counter})

    # B. Restart the bridge; the learned recipe must survive.
    before = counter()
    subprocess.run(["systemctl", "--user", "restart", "replant-bridge"], check=True)
    time.sleep(1.5)
    receipt = post(LOOP, {"request": req, "record": rec})
    assert receipt["status"] == "succeeded" and receipt["operation"] == "replay", receipt
    assert counter() == before + 1
    results.append({"claim": "recipe survives bridge restart", "operation": receipt["operation"]})

    # C. Same prompt/input, changed context => miss, no action.
    before = counter()
    receipt = post(RUN, request_for(context + "-changed"))
    assert receipt["status"] == "miss", receipt
    assert counter() == before
    results.append({"claim": "changed context misses", "status": receipt["status"]})

    # D. Same context, changed input => miss, no action.
    before = counter()
    receipt = post(RUN, request_for(context, amount=2))
    assert receipt["status"] == "miss", receipt
    assert counter() == before
    results.append({"claim": "changed input misses", "status": receipt["status"]})

    # E. Changed tool implementation invalidates; restoring it replays again.
    original = TOOLS.read_text()
    before = counter()
    TOOLS.write_text(original + "\n# implementation drift for claims test\n")
    receipt = post(RUN, req)
    assert receipt["status"] == "miss", receipt
    assert counter() == before, "drifted implementation must not execute"
    TOOLS.write_text(original)
    receipt = post(RUN, req)
    assert receipt["status"] == "succeeded" and receipt["operation"] == "replay", receipt
    assert counter() == before + 1
    results.append({"claim": "implementation drift invalidates, restore replays",
                    "drift": "miss", "restored": receipt["operation"]})

    # F. Failed check blocks reuse; explicit re-record recovers.
    fail_context = "fail-" + stamp
    fail_req = request_for(fail_context)
    fail_rec = record_for(fail_req)
    marker = COUNTER.parent / "fail"
    marker.write_text("fail")
    before = counter()
    receipt = post(RECORD, fail_rec)
    assert receipt["status"] == "failed", receipt
    assert counter() == before + 1, "the action ran before the check failed (no rollback)"
    marker.unlink()
    receipt = post(RUN, fail_req)
    assert receipt["status"] == "blocked", receipt
    assert counter() == before + 1, "blocked replay must not execute"
    receipt = post(RUN, fail_req)
    assert receipt["status"] == "blocked", receipt
    receipt = post(RECORD, fail_rec)
    assert receipt["status"] == "succeeded" and receipt["operation"] == "record", receipt
    assert counter() == before + 2
    receipt = post(RUN, fail_req)
    assert receipt["status"] == "succeeded" and receipt["operation"] == "replay", receipt
    assert counter() == before + 3
    results.append({"claim": "failed check blocks; explicit re-record recovers",
                    "failed": "failed", "next": "blocked", "re_record": "record", "replay": "replay"})

    # G. Stats reflect completed records/replays, not assumptions.
    live = stats()
    results.append({"claim": "stats are measured", "verified_records": live["verified_records"],
                    "verified_replays": live["verified_replays"], "failures": live["failures"],
                    "blocked": live["blocked"], "pending": live["pending"]})

    print(json.dumps({"status": "passed", "results": results}, indent=2))


if __name__ == "__main__":
    main()
