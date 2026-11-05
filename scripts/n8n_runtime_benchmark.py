#!/usr/bin/env python3
"""Measure a real n8n Loop Over Items with HTTP model calls inside the workflow.

Requires a working n8n CLI and built target/release/replant. Uses isolated n8n
and Replant state, never the deployed instance. Directory must not exist.
Local gateway defaults and planner prompt match llm_cost_benchmark.py.
"""
import argparse
import json
import os
import pathlib
import secrets
import socket
import subprocess
import time
import urllib.request
import uuid

from llm_cost_benchmark import LLM_MODEL, LLM_URL, MAX_TOKENS, SYSTEM, USER

ROOT = pathlib.Path(__file__).resolve().parents[1]


def free_port(host):
    with socket.socket() as sock:
        sock.bind((host, 0))
        return sock.getsockname()[1]


def workflow(kind, iterations, bridge, context):
    nodes, connections = [], {}

    def node(name, kind, version=1, **parameters):
        nodes.append({"id": str(uuid.uuid4()), "name": name, "type": "n8n-nodes-base." + kind,
                      "typeVersion": version, "position": [len(nodes) * 180, 0],
                      "parameters": parameters, "retryOnFail": False})

    def link(source, target, output=0):
        outputs = connections.setdefault(source, {"main": []})["main"]
        while len(outputs) <= output:
            outputs.append([])
        outputs[output].append({"node": target, "type": "main", "index": 0})

    def http(name, url, body, auth=False):
        parameters = {"method": "POST", "url": url, "sendBody": True,
                      "specifyBody": "json", "jsonBody": body, "options": {"timeout": 120000}}
        if auth:
            parameters.update(sendHeaders=True, headerParameters={"parameters": [
                {"name": "Authorization", "value": "=Bearer {{$env.REPLANT_BENCH_TOKEN}}"}]})
        node(name, "httpRequest", 4.2, **parameters)

    request = {"prompt": "increment once", "input": {"amount": 1}, "context": {"workflow": context}}
    node("Start", "manualTrigger")
    node("Input", "set", 3.4, mode="raw", jsonOutput=json.dumps({
        "request": request, "items": [{"request": request, "iteration": i + 1} for i in range(iterations)]}), options={})
    node("Items", "splitOut", 1, fieldToSplitOut="items", include="noOtherFields", options={})
    node("Loop Over Items", "splitInBatches", 3, batchSize=1, options={})
    node("Done", "noOp")
    http("LLM Plan", LLM_URL, json.dumps({"model": LLM_MODEL, "temperature": 0, "max_tokens": MAX_TOKENS,
         "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": USER}]}))
    http("Record", bridge + "/record", '={{ JSON.stringify({request: $("Input").first().json.request, '
         'recipe: JSON.parse($json.choices[0].message.content.replace(/^```(?:json)?\\s*/, "").replace(/\\s*```$/, "")), '
         'trace: {provider: "n8n-live-token-benchmark", usage: $json.usage} }) }}', True)
    link("Start", "Input")
    link("Input", "Items")
    link("Items", "Loop Over Items")
    link("Loop Over Items", "Done", 0)
    link("LLM Plan", "Record")
    link("Record", "Loop Over Items")
    if kind == "baseline":
        link("Loop Over Items", "LLM Plan", 1)
    else:
        http("Lookup", bridge + "/run", "={{$json.request}}", True)
        def condition(status):
            return {"conditions": {"options": {"caseSensitive": True, "leftValue": "",
                    "typeValidation": "strict", "version": 2}, "conditions": [{
                    "id": str(uuid.uuid4()), "leftValue": "={{$json.status}}", "rightValue": status,
                    "operator": {"type": "string", "operation": "equals"}}], "combinator": "and"}}
        node("Route", "switch", 3, rules={"values": [condition("succeeded"), condition("miss")]},
             options={"fallbackOutput": "extra"})
        node("Unsafe Stop", "noOp")
        link("Loop Over Items", "Lookup", 1)
        link("Lookup", "Route")
        link("Route", "Loop Over Items", 0)
        link("Route", "LLM Plan", 1)
        link("Route", "Unsafe Stop", 2)
    return {"id": uuid.uuid4().hex[:16], "name": "Replant token loop " + kind,
            "nodes": nodes, "connections": connections, "active": False, "settings": {"executionOrder": "v1"}}


def execution_result(text):
    decoder = json.JSONDecoder()
    for line_start in [0] + [i + 1 for i, c in enumerate(text) if c == "\n"]:
        if text[line_start:line_start + 1] != "{":
            continue
        try:
            value, _ = decoder.raw_decode(text[line_start:])
        except ValueError:
            continue
        if isinstance(value, dict) and "resultData" in value.get("data", {}):
            return value
    raise RuntimeError("n8n did not emit execution data; inspect private execution log")


def main():
    if not __debug__:
        raise RuntimeError("Run without -O: verification assertions must remain enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iterations", type=int, default=100)
    parser.add_argument("--directory", type=pathlib.Path, required=True)
    parser.add_argument("--host", required=True, help="One private interface IP reachable by n8n; never 0.0.0.0")
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("iterations must be positive")
    os.umask(0o077)
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    host, port = args.host, free_port(args.host)
    bridge = f"http://{host}:{port}"
    env = dict(os.environ, REPLANT_API_TOKEN=secrets.token_urlsafe(32))
    env.update(REPLANT_BENCH_TOKEN=env["REPLANT_API_TOKEN"], N8N_USER_FOLDER=str(directory / "n8n"),
               DB_TYPE="sqlite", DB_SQLITE_DATABASE=str(directory / "n8n.sqlite"),
               EXECUTIONS_MODE="regular", N8N_ENCRYPTION_KEY=secrets.token_hex(32),
               N8N_DIAGNOSTICS_ENABLED="false", N8N_VERSION_NOTIFICATIONS_ENABLED="false",
               N8N_TEMPLATES_ENABLED="false", N8N_COMMUNITY_PACKAGES_ENABLED="false",
               N8N_RUNNERS_MODE="external", N8N_RUNNERS_BROKER_LISTEN_ADDRESS=host,
               N8N_RUNNERS_AUTH_TOKEN=secrets.token_urlsafe(32),
               N8N_RUNNERS_BROKER_PORT=str(free_port(host)), N8N_LISTEN_ADDRESS=host,
               N8N_BLOCK_ENV_ACCESS_IN_NODE="false", N8N_LOG_LEVEL="info")
    tools = {}
    for name, kind in [("increment", "action"), ("verify", "check")]:
        tools[name] = {"kind": kind, "program": "/usr/bin/python3",
                       "args": [str(ROOT / "examples/fixture_tools.py"), name, str(directory)],
                       "fingerprint_files": [str(ROOT / "examples/fixture_tools.py")], "timeout_ms": 2000}
    (directory / "tools.json").write_text(json.dumps(tools))
    process = subprocess.Popen([str(ROOT / "target/release/replant"), "serve", "--store", str(directory / "state"),
                               "--tools", str(directory / "tools.json"), "--bind", f"{host}:{port}"], env=env,
                               stdout=subprocess.DEVNULL)
    report = {"scope": "actual n8n CLI workflow, Loop Over Items and HTTP Request model node",
              "model": LLM_MODEL, "iterations": args.iterations, "status": "running"}
    try:
        for attempt in range(50):
            try:
                with urllib.request.urlopen(bridge + "/healthz", timeout=1):
                    break
            except Exception:
                if process.poll() is not None:
                    raise RuntimeError("test bridge failed to start")
                time.sleep(.05)
        else:
            raise RuntimeError("test bridge did not become reachable")
        for kind in ["baseline", "replant"]:
            flow = workflow(kind, args.iterations, bridge, f"n8n-{kind}-{uuid.uuid4().hex}")
            path = directory / f"{kind}.json"
            path.write_text(json.dumps(flow, indent=2))
            with (directory / f"{kind}-import.log").open("w") as log:
                subprocess.run(["n8n", "import:workflow", "--input", str(path)], env=env, stdout=log,
                               stderr=subprocess.STDOUT, timeout=120, check=True)
            counter = directory / "counter"
            before = int(counter.read_text()) if counter.exists() else 0
            started = time.perf_counter()
            output = directory / f"{kind}-execution.log"
            with output.open("w") as log:
                subprocess.run(["n8n", "execute", "--id", flow["id"], "--rawOutput"], env=env,
                               stdout=log, stderr=subprocess.STDOUT, timeout=1000, check=True)
            elapsed = time.perf_counter() - started
            result = execution_result(output.read_text())
            assert not result["data"]["resultData"].get("error"), "n8n execution failed; inspect private log"
            data = result["data"]["resultData"]["runData"]
            assert not data.get("Unsafe Stop"), "unsafe status reached stop branch"
            llm = [item["json"] for run in data["LLM Plan"] for item in run["data"]["main"][0]]
            done = data["Done"][0]["data"]["main"][0]
            assert len(done) == args.iterations, (kind, "loop incomplete", len(done))
            assert all(item["json"]["status"] == "succeeded" for item in done)
            operations = [item["json"]["operation"] for item in done]
            assert operations == (["record"] * args.iterations if kind == "baseline" else ["record"] + ["replay"] * (args.iterations - 1))
            assert len(llm) == (args.iterations if kind == "baseline" else 1)
            usages = []
            for response in llm:
                usage = response["usage"]
                assert all(isinstance(usage[field], int) and usage[field] >= 0
                           for field in ["prompt_tokens", "completion_tokens", "total_tokens"])
                assert usage["total_tokens"] > 0
                usages.append({field: usage[field] for field in ["prompt_tokens", "completion_tokens", "total_tokens"]})
            assert int(counter.read_text()) - before == args.iterations
            checks = [json.loads(line) for line in (directory / "checks.jsonl").read_text().splitlines()]
            arm_checks = checks[before:]
            assert arm_checks == [{"ok": True, "count": before + i} for i in range(1, args.iterations + 1)]
            report[kind] = {"llm_calls": len(llm), "usages": usages, "operations": operations,
                            "tokens": {field: sum(u[field] for u in usages) for field in usages[0]},
                            "actions": args.iterations, "checks": len(arm_checks), "elapsed_s": round(elapsed, 3)}
            print(json.dumps({kind: {k: v for k, v in report[kind].items() if k not in ["usages", "operations"]}}), flush=True)
            (directory / "report.json").write_text(json.dumps(report, indent=2))
        report["token_reduction_percent"] = 100 * (1 - report["replant"]["tokens"]["total_tokens"] / report["baseline"]["tokens"]["total_tokens"])
        report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        process.terminate()
        process.wait(timeout=10)
        (directory / "report.json").write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
