#!/usr/bin/env python3
"""Measure actual LLM token use: baseline vs physis-replant crystallization.

Baseline: every iteration calls the LLM and records/executes the plan.
Replant:  first iteration calls the LLM once; later iterations replay.
Both arms execute the same real counter action and check every iteration.

The LLM is a local llama.cpp gateway on omo (free); token counts are real API
usage. Dollar figures are an explicitly labeled example price, not billed here.
"""
import json
import os
import pathlib
import time
import urllib.request

LLM_URL = os.environ.get("REPLANT_LLM_URL", "http://100.116.39.57:8088/v1/chat/completions")
LLM_MODEL = os.environ.get("REPLANT_LLM_MODEL", "gemma4-e2b-heretic-Q4_K_M")
WEBHOOK = os.environ.get("REPLANT_N8N_WEBHOOK", "http://127.0.0.1:5678/webhook")
LOOP = WEBHOOK + "/replant-claims"
RECORD = WEBHOOK + "/replant-claims-record"
COUNTER = pathlib.Path(os.environ.get("REPLANT_COUNTER", "/home/gio/.replant-n8n/counter/counter"))
N = int(os.environ.get("REPLANT_ITERATIONS", "5"))
INPUT_PRICE = float(os.environ.get("REPLANT_INPUT_PRICE", "0.10"))   # USD / Mtok, example
OUTPUT_PRICE = float(os.environ.get("REPLANT_OUTPUT_PRICE", "0.30")) # USD / Mtok, example

SYSTEM = (
    "You are a planner. Return ONLY a JSON object, no prose. Shape: "
    '{"steps":[{"tool":"increment","input":{"amount":1}}],'
    '"checks":[{"tool":"verify","input":{}}]}'
)
USER = "Increment the counter once and verify it. Tools: increment(amount=1), verify()."


def http_json(url, payload):
    req = urllib.request.Request(
        url, data=json.dumps(payload).encode(), headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=120) as response:
        return json.loads(response.read())


def llm_plan():
    response = http_json(LLM_URL, {
        "model": LLM_MODEL,
        "temperature": 0,
        "max_tokens": 800,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": USER},
        ],
    })
    content = response["choices"][0]["message"].get("content") or ""
    text = content.strip()
    if text.startswith("```"):
        text = text.split("```")[1]
        if text.startswith("json"):
            text = text[4:]
    plan = json.loads(text)
    recipe = {"steps": plan["steps"], "checks": plan["checks"]}
    usage = response.get("usage", {})
    return recipe, {
        "prompt_tokens": usage.get("prompt_tokens", 0),
        "completion_tokens": usage.get("completion_tokens", 0),
        "total_tokens": usage.get("total_tokens", 0),
    }


def request_for(context):
    return {"prompt": "increment in n8n", "input": {"amount": 1}, "context": {"workflow": context}}


def record_for(request, recipe):
    return {"request": request, "recipe": recipe, "trace": {"provider": "llm-cost-benchmark"}}


def counter():
    return int(COUNTER.read_text())


def arm(kind, stamp):
    context = f"llm-{kind}-{stamp}"
    request = request_for(context)
    start_counter = counter()
    calls = 0
    prompt_tokens = 0
    completion_tokens = 0
    total_tokens = 0
    operations = []
    started = time.perf_counter()
    for iteration in range(N):
        if kind == "replant" and iteration > 0:
            receipt = http_json(LOOP, {"request": request, "record": record_for(request, recipe)})
        else:
            recipe, usage = llm_plan()
            calls += 1
            prompt_tokens += usage["prompt_tokens"]
            completion_tokens += usage["completion_tokens"]
            total_tokens += usage["total_tokens"]
            payload = {"request": request, "record": record_for(request, recipe)}
            receipt = http_json(LOOP if kind == "replant" else RECORD, payload if kind == "replant" else payload["record"])
        assert receipt["status"] == "succeeded", receipt
        operations.append(receipt["operation"])
    elapsed = time.perf_counter() - started
    delta = counter() - start_counter
    assert delta == N, (kind, delta, N)
    cost = (prompt_tokens / 1e6 * INPUT_PRICE) + (completion_tokens / 1e6 * OUTPUT_PRICE)
    return {
        "arm": kind,
        "iterations": N,
        "llm_calls": calls,
        "operations": operations,
        "counter_delta": delta,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": total_tokens,
        "example_cost_usd": round(cost, 8),
        "elapsed_s": round(elapsed, 3),
    }


def main():
    stamp = str(int(time.time()))
    baseline = arm("baseline", stamp)
    replant = arm("replant", stamp)
    saved_tokens = baseline["total_tokens"] - replant["total_tokens"]
    saved_calls = baseline["llm_calls"] - replant["llm_calls"]
    saved_cost = baseline["example_cost_usd"] - replant["example_cost_usd"]
    report = {
        "llm": {"url": LLM_URL, "model": LLM_MODEL, "note": "local free gateway; real token usage"},
        "pricing": {"input_usd_per_mtok": INPUT_PRICE, "output_usd_per_mtok": OUTPUT_PRICE,
                    "note": "example hosted price; not billed in this test"},
        "baseline": baseline,
        "replant": replant,
        "saved": {
            "llm_calls": saved_calls,
            "prompt_tokens": baseline["prompt_tokens"] - replant["prompt_tokens"],
            "completion_tokens": baseline["completion_tokens"] - replant["completion_tokens"],
            "total_tokens": saved_tokens,
            "token_reduction_percent": round(100.0 * saved_tokens / baseline["total_tokens"], 1)
            if baseline["total_tokens"] else None,
            "example_cost_usd": round(saved_cost, 8),
        },
    }
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
