#!/usr/bin/env python3
"""Exercise issue #1 contracts against a real Replant binary and real effects.

Creates a new private output directory. No live VM/customer workflow/model calls.
Pass --long-timeout-proof to include one real 121-second action.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request


def fixture(mode: str, root: Path) -> None:
    payload = json.load(sys.stdin)
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    with (root / "effects.jsonl").open("a") as stream:
        stream.write(json.dumps({"mode": mode, "input": payload}) + "\n")
    state = root / "state.json"
    if mode == "apply":
        if payload.get("sleep_seconds"):
            time.sleep(payload["sleep_seconds"])
        state.write_text(json.dumps({"days": payload["days"]}))
        print(json.dumps({"ok": True, "days": payload["days"]}), flush=True)
    elif mode == "check":
        observed = json.loads(state.read_text())
        ok = observed["days"] == payload["days"]
        print(json.dumps({"ok": ok, "observed": observed}), flush=True)
        raise SystemExit(0 if ok else 1)
    elif mode == "fail":
        print(json.dumps({"ok": False, "error": "seed fixture deliberately failed"}), flush=True)
        print("independent check diagnostic tail", file=sys.stderr, flush=True)
        raise SystemExit(1)
    else:
        raise ValueError("unknown fixture mode")


class Acceptance:
    def __init__(self, binary: Path, root: Path, host: str):
        self.binary, self.root, self.host = binary.resolve(), root.resolve(), host
        self.root.mkdir(mode=0o700)
        self.effects = self.root / "fixture"
        self.marker = self.root / "implementation-marker.txt"
        self.marker.write_text("v1\n")
        self.registry = self.root / "tools.json"
        script = str(Path(__file__).resolve())
        def tool(mode, **roles):
            return {**roles, "program": str(Path(sys.executable).resolve()),
                    "args": [script, "--fixture", mode, str(self.effects)],
                    "fingerprint_files": [script, str(self.marker)], "timeout_ms": 2000}
        self.tools = {"apply": tool("apply", kinds=["action", "undo"]),
                      "check": tool("check", kind="check"),
                      "fail": tool("fail", kind="check")}
        self.registry.write_text(json.dumps(self.tools))
        self.store = self.root / "cli-state"
        self.request = {"prompt": "set the requested number of seed days for the isolated fixture then independently verify the persisted state",
                        "input": {"days": 3}, "context": {"fixture": "issue1"}}
        self.recipe = {"bind_inputs": True,
                       "steps": [{"tool": "apply", "input": {"days": {"$input": "days"}}}],
                       "checks": [{"tool": "check", "input": {"days": {"$input": "days"}}}],
                       "undo": [{"tool": "apply", "input": {"days": 0}}]}
        self.report = {"binary": str(self.binary),
                       "binary_sha256": hashlib.sha256(self.binary.read_bytes()).hexdigest(),
                       "checks": [], "receipts": []}

    def count(self):
        path = self.effects / "effects.jsonl"
        return len(path.read_text().splitlines()) if path.exists() else 0

    def days(self):
        return json.loads((self.effects / "state.json").read_text())["days"]

    def record_input(self, request=None, recipe=None):
        return {"request": request or self.request, "recipe": recipe or self.recipe,
                "trace": {"planner": "explicit issue1 acceptance fixture"}}

    def cli(self, command, value=None, *, success=True, store=None, timeout=30):
        args = [str(self.binary), command, "--store", str(store or self.store), "--tools", str(self.registry)]
        proc = subprocess.run(args, input=json.dumps(value) if value is not None else "",
                              text=True, capture_output=True, timeout=timeout)
        result = json.loads(proc.stdout)
        assert (proc.returncode == 0) == success, (command, proc.returncode, result, proc.stderr)
        self.report["receipts"].append({"transport": "cli", "command": command, "exit": proc.returncode, "result": result})
        return result

    def checked(self, label):
        self.report["checks"].append(label)

    def drift(self):
        with self.marker.open("a") as stream:
            stream.write("operator implementation dependency changed\n")

    def cli_contracts(self):
        first = self.cli("record", self.record_input())
        assert first["status"] == "succeeded" and self.days() == 3
        assert first["calls"][0]["kind"] == "action"
        before = self.count()
        replay = self.cli("run", self.request)
        assert replay["operation"] == "replay" and replay["provider_calls"] == 0
        assert self.count() == before + 2 and self.days() == 3
        self.checked("CLI record and exact replay re-execute actual actions and independent checks")

        changed = {**self.request, "input": {"days": 4}}
        before = self.count()
        review = self.cli("run", changed)
        assert review["status"] == "review" and self.count() == before
        approved = self.cli("approve", {"request": changed, "candidate": first["id"]})
        assert approved["status"] == "succeeded" and self.days() == 4
        assert approved["calls"][0]["output"]["days"] == 4
        undone = self.cli("undo", {"id": approved["id"]})
        assert undone["status"] == "succeeded" and self.days() == 0
        assert undone["calls"][0]["kind"] == "undo"
        before = self.count()
        assert self.cli("run", changed, success=False)["status"] == "blocked"
        assert self.count() == before
        self.checked("near-match does not execute; explicit approval binds new input; multi-role undo blocks replay")

        self.drift()
        before = self.count()
        stale = self.cli("run", self.request)
        assert stale["status"] == "stale" and self.count() == before
        assert stale["stale_record"]["id"] == first["id"]
        assert stale["stale_record"]["recipe"]["steps"] == self.recipe["steps"]
        renewed = self.cli("rerecord", {"id": first["id"]})
        assert renewed["status"] == "succeeded" and renewed["operation"] == "rerecord"
        assert renewed["id"] != first["id"] and self.days() == 3
        assert self.count() == before + 2
        assert self.cli("run", self.request)["operation"] == "replay"
        before = self.count()
        self.cli("rerecord", {"id": first["id"]}, success=False)
        assert self.count() == before
        self.checked("stale lookup preserves template with zero effects; explicit checked rerecord replaces index; obsolete id refuses")

        malformed = {**self.recipe, "undo": [{"tool": "apply", "input": {"days": {"$input": "missing"}}}]}
        new_request = {**self.request, "context": {"fixture": "invalid-later-undo"}}
        before = self.count()
        self.cli("record", self.record_input(new_request, malformed), success=False)
        assert self.count() == before
        self.checked("malformed later undo binding refuses before any earlier action")

        failure_recipe = {"bind_inputs": True, "steps": self.recipe["steps"],
                          "checks": [{"tool": "fail", "input": {}}]}
        failure_request = {**self.request, "context": {"fixture": "failure-diagnostics"}}
        failed = self.cli("record", self.record_input(failure_request, failure_recipe), success=False)
        call = failed["calls"][-1]
        assert failed["status"] == "failed" and call["exit_code"] == 1
        assert call["output"]["ok"] is False and "deliberately failed" in call["stdout"]
        assert "diagnostic tail" in call["stderr"]
        before = self.count()
        assert self.cli("run", failure_request, success=False)["status"] == "blocked"
        assert self.count() == before
        stats = self.cli("stats")
        assert stats["stale"] >= 1 and stats["failures"] >= 1
        self.checked("failed real check retains JSON/stdout/stderr/exit=1 and prevents subsequent replay")

    def http_contracts(self):
        spec = importlib.util.spec_from_file_location("replant_client", Path(__file__).resolve().parents[1] / "sdk/python/replant_client.py")
        assert spec is not None and spec.loader is not None, "Python SDK module not found"
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with socket.socket() as listener:
            listener.bind((self.host, 0))
            port = listener.getsockname()[1]
        url = f"http://{self.host}:{port}"
        os.environ["NO_PROXY"] = os.environ.get("NO_PROXY", "") + "," + self.host
        token = secrets.token_urlsafe(32)
        env = {**os.environ, "REPLANT_API_TOKEN": token}
        args = [str(self.binary), "serve", "--bind", f"{self.host}:{port}", "--store", str(self.root / "http-state"), "--tools", str(self.registry)]
        with (self.root / "http-server.log").open("w") as log:
            server = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            try:
                client = module.ReplantClient(url, token, timeout=10)
                for _ in range(100):
                    if server.poll() is not None:
                        raise RuntimeError("HTTP bridge exited before readiness")
                    try:
                        assert client.health()["status"] == "ok"
                        break
                    except (module.ReplantError, OSError):
                        time.sleep(0.05)
                else:
                    raise RuntimeError("HTTP bridge did not become ready")
                first = client.record(self.record_input())
                assert first["status"] == "succeeded" and self.days() == 3
                self.drift()
                before = self.count()
                stale = client.run(self.request)
                assert stale["status"] == "stale" and self.count() == before
                renewed = client.rerecord(stale["stale_record"]["id"])
                assert renewed["operation"] == "rerecord" and renewed["status"] == "succeeded"
                assert self.count() == before + 2 and self.days() == 3
                self.report["receipts"].append({"transport": "python-sdk-http", "result": renewed})
                try:
                    module.ReplantClient(url, "incorrect-token").rerecord(renewed["id"])
                    raise AssertionError("unauthorized rerecord accepted")
                except module.ReplantError as error:
                    assert error.status == 401
                assert self.count() == before + 2, "unauthorized re-record caused effects"
                self.checked("real authenticated HTTP and Python SDK stale/rerecord; unauthorized mutation refused")

                ts = (Path(__file__).resolve().parents[1] / "sdk/typescript/replant.mjs").as_uri()
                js = "import { ReplantClient } from " + json.dumps(ts) + "; const client = new ReplantClient(process.env.REPLANT_TEST_URL, process.env.REPLANT_API_TOKEN); console.log(JSON.stringify(await client.run(JSON.parse(process.env.REPLANT_TEST_REQUEST))));"
                node = subprocess.run(["node", "--input-type=module", "-e", js], env={**env, "REPLANT_TEST_URL": url, "REPLANT_TEST_REQUEST": json.dumps(self.request)}, capture_output=True, text=True, timeout=30)
                assert node.returncode == 0, node.stderr
                receipt = json.loads(node.stdout)
                assert receipt["operation"] == "replay" and receipt["status"] == "succeeded"
                assert self.count() == before + 4 and self.days() == 3
                self.report["receipts"].append({"transport": "typescript-sdk-http", "result": receipt})
                self.checked("real TypeScript SDK HTTP replay against built bridge")
            finally:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)

    def mcp_contracts(self):
        store = self.root / "mcp-state"
        first = self.cli("record", self.record_input(), store=store)
        self.drift()
        messages = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "replant_run", "arguments": {"request": self.request}}},
            {"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "replant_rerecord", "arguments": {"id": first["id"]}}},
            {"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "replant_run", "arguments": {"request": self.request}}},
        ]
        before = self.count()
        proc = subprocess.run([str(self.binary), "mcp", "--store", str(store), "--tools", str(self.registry)], input="\n".join(json.dumps(row) for row in messages) + "\n", text=True, capture_output=True, timeout=30)
        assert proc.returncode == 0, proc.stderr
        rows = list(map(json.loads, proc.stdout.splitlines()))
        assert len(rows) == 5, f"unexpected MCP stdout frames: {rows!r}"
        assert all(row.get("jsonrpc") == "2.0" and "id" in row for row in rows)
        responses = {row["id"]: row for row in rows}
        assert set(responses) == {1, 2, 3, 4, 5}, "duplicate or missing MCP response ids"
        assert len(responses[2]["result"]["tools"]) == 8
        for identifier, status, operation in [(3, "stale", "lookup"), (4, "succeeded", "rerecord"), (5, "succeeded", "replay")]:
            response = responses[identifier]["result"]
            assert response["isError"] is False
            receipt = json.loads(response["content"][0]["text"])
            assert receipt["status"] == status and receipt["operation"] == operation
            self.report["receipts"].append({"transport": "mcp", "result": receipt})
        assert self.days() == 3 and self.count() == before + 4
        self.checked("real MCP stdio advertises eight tools and projects stale/rerecord/replay")

    def long_timeout(self):
        self.tools["apply"]["timeout_ms"] = 180000
        self.registry.write_text(json.dumps(self.tools))
        recipe = {"steps": [{"tool": "apply", "input": {"days": 9, "sleep_seconds": 121}}],
                  "checks": [{"tool": "check", "input": {"days": 9}}]}
        request = {"prompt": "one real action beyond the former 120 second cap", "input": {}, "context": {"fixture": "long-deadline"}}
        start = time.monotonic()
        result = self.cli("record", self.record_input(request, recipe), store=self.root / "long-state", timeout=200)
        elapsed = time.monotonic() - start
        assert result["status"] == "succeeded" and elapsed > 120 and self.days() == 9
        self.report["long_action_elapsed_seconds"] = elapsed
        self.checked("real action exceeds 120 seconds and passes an independent state check under explicit 180000ms deadline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path, help="new private output directory; must not already exist")
    parser.add_argument("--host", required=True, help="one local interface reachable by the test clients, not 0.0.0.0")
    parser.add_argument("--long-timeout-proof", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    test = Acceptance(args.binary, args.out, args.host)
    try:
        test.cli_contracts()
        test.http_contracts()
        test.mcp_contracts()
        if args.long_timeout_proof:
            test.long_timeout()
        test.report["status"] = "PASS"
    except Exception as error:
        test.report["status"] = "FAIL"
        test.report["error"] = repr(error)
        raise
    finally:
        test.report["effect_log_entries"] = test.count()
        (test.root / "report.json").write_text(json.dumps(test.report, indent=2))
        print(json.dumps({"status": test.report["status"], "checks": test.report["checks"], "report": str(test.root / "report.json"), "long_action_elapsed_seconds": test.report.get("long_action_elapsed_seconds")}))


if __name__ == "__main__":
    if sys.argv[1:2] == ["--fixture"]:
        fixture(sys.argv[2], Path(sys.argv[3]))
    else:
        main()
