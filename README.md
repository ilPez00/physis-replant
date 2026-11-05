# physis-replant

### Stop paying an LLM to do the same task twice.

Record a verified tool recipe once. Re-run the **real actions and checks** forever after — no inference, no cached side-effect responses. Local, auditable, and deliberately narrow.

**Measured inside n8n: 100 iterations, 99% fewer LLM tokens (100 calls vs 1; 45,200 vs 452 tokens).** Both arms executed 100 real actions and 100 independent checks.

[![CI](https://github.com/ilPez00/physis-replant/actions/workflows/ci.yml/badge.svg)](https://github.com/ilPez00/physis-replant/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-1.89%2B-orange)
![Status](https://img.shields.io/badge/status-experimental-yellow)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)

---

## The problem

Agents are great at figuring out a task once and terrible at remembering it.

A recurring workflow — sync a CRM, reconcile a file, restart a service, build a report — pays the model again on every run. Prompt caching discounts the prefix, but the model still runs, still emits tokens, and can still choose a different tool call. Workflow-memory tools inject a plan into the prompt; the model still gets called.

`replant` records the **typed tool recipe** behind a successful run, verifies it with independent checks, and re-executes that recipe on later identical requests. The model is not consulted again until something actually changes.

## How it works

```
first request ──▶ your LLM/planner ──▶ typed recipe ──▶ execute actions ──▶ independent checks
                                                              │
                                                              ▼
                                                     persist + receipt
later request ──▶ exact request match? ──▶ re-execute actions ──▶ checks ──▶ receipt
                         │ no
                         ▼
                    status: miss  (call the planner again)
```

The engine never returns a cached tool result as if the action happened. It runs the action and the check again, every time.

## What gets remembered

- the exact `{prompt, input, context}` request,
- the structured cognition/provenance trace,
- the typed recipe (registered tool names + JSON inputs),
- the tool/registry/engine fingerprints that produced it,
- every record/replay decision in an append-only receipt journal.

A replay is denied when the request, context, tool implementation, registry, or engine changed; when a prior execution failed or was interrupted; or when the journal/index is inconsistent. Failures are errors or `blocked`, never a silent retry of partial work.

## Quickstart

```sh
cargo build --release

# 1. Register trusted tools. A model may name them, never choose an executable.
cat > tools.json <<'JSON'
{
  "increment": {
    "kind": "action",
    "program": "/usr/bin/python3",
    "args": ["examples/fixture_tools.py", "increment", "/tmp/replant-demo"],
    "fingerprint_files": ["examples/fixture_tools.py"],
    "timeout_ms": 2000
  },
  "verify": {
    "kind": "check",
    "program": "/usr/bin/python3",
    "args": ["examples/fixture_tools.py", "verify", "/tmp/replant-demo"],
    "fingerprint_files": ["examples/fixture_tools.py"],
    "timeout_ms": 2000
  }
}
JSON

# 2. Ask: does this exact task already have a verified recipe?
echo '{"prompt":"increment once","input":{"amount":1},"context":{"workflow":"demo-v1"}}' \
  | replant run --store /tmp/replant-state --tools tools.json
# -> {"status":"miss", ...}

# 3. Let the planner supply a recipe once. record executes and checks it.
echo '{"request":{...},"recipe":{"steps":[{"tool":"increment","input":{"amount":1}}],"checks":[{"tool":"verify","input":{}}]},"trace":{...}}' \
  | replant record --store /tmp/replant-state --tools tools.json
# -> {"status":"succeeded","operation":"record", ...}

# 4. Repeat the same request. No planner. The action and check run again.
echo '{"prompt":"increment once","input":{"amount":1},"context":{"workflow":"demo-v1"}}' \
  | replant run --store /tmp/replant-state --tools tools.json
# -> {"status":"succeeded","operation":"replay","provider_calls":0, ...}
```

Full request/record shapes and the n8n adapter: [`examples/n8n/README.md`](examples/n8n/README.md).

## Measured inside a real n8n loop

**n8n 2.46.5**, Loop Over Items (batch size 1), local
`gemma4-e2b-heretic-Q4_K_M`. The **HTTP Request model node runs inside n8n**.
Python only imports/starts the workflows and checks their results; it does not
call the model in this test. Both arms start without a matching recipe.

Task: increment a real file counter once, then independently verify the change.
Baseline plans every iteration. Replant looks up the exact request first,
plans/records on the first miss, then re-executes the verified recipe.

| Completed iterations per arm | LLM calls: baseline / Replant | Baseline tokens | Replant tokens | Tokens saved | Saved |
|---:|---:|---:|---:|---:|---:|
| 1 | 1 / 1 | 452 | 452 | 0 | 0.0% |
| 5 | 5 / 1 | 2,260 | 452 | 1,808 | **80.0%** |
| 10 | 10 / 1 | 4,520 | 452 | 4,068 | **90.0%** |
| 25 | 25 / 1 | 11,300 | 452 | 10,848 | **96.0%** |
| 50 | 50 / 1 | 22,600 | 452 | 22,148 | **98.0%** |
| **100** | **100 / 1** | **45,200** | **452** | **44,748** | **99.0%** |

Rows are measured cumulative checkpoints from the same two 100-item executions,
not extrapolations or separate trials. **Both arms completed 100 real actions
and 100 successful independent checks.** Replant recorded once and replayed 99
times; no actions were replaced by cached-success responses.

Totals come from every model response's required usage telemetry, including
reported prompt-cache tokens. Retries were disabled. This measures tokens and
avoided inference calls, **not paid invoice savings**. It proves exact-repeat
reuse, not arbitrary changing tasks. A fixed workflow written by hand can skip
the LLM entirely; Replant addresses recipes initially supplied by an agent.

Reproduce with [`scripts/n8n_runtime_benchmark.py`](scripts/n8n_runtime_benchmark.py)
and the [n8n instructions](examples/n8n/README.md#actual-n8n-loop-with-a-live-model).
See the [runtime report](docs/n8n-runtime-2026-11-05.md) and
[machine-readable results](docs/n8n-runtime-2026-11-05.json).

Earlier OpenRouter measurement: 16,212 vs 162 tokens over 100 iterations using
`nvidia/nemotron-3-ultra-550b-a55b:free`. That older test called the planner from
Python, not from an n8n model node; see
[`docs/verification-2026-11-04.md`](docs/verification-2026-11-04.md).

## Not another cache

| | Prompt cache | Response replay<br>(agentrr, tracefork) | Workflow memory<br>(AWM) | **replant** |
|---|---|---|---|---|
| Repeats the real action? | no | no — returns recorded bytes | no — injects a plan into the prompt | **yes** |
| Runs independent checks before reuse? | no | no | no | **yes** |
| Invalidates on tool implementation change? | no | no | no | **yes** |
| Works for side-effecting tools? | n/a | no (replay must avoid side effects) | n/a | **yes, explicitly** |
| Local / self-hosted? | provider-side | varies | varies | **yes** |
| Audit trail for every decision? | no | partial | no | **yes** |

Record/replay is a crowded space for **debugging** (agentrr, tracefork, hetu, agent-blackbox) and for **browser actions** (Stagehand/Browserbase). `replant` is for the narrower, harder case: repeated side-effecting automation where a stale or wrong replay is worse than paying for a model call.

## Safety model

- **Operator-owned tools.** A model chooses only a registered name and JSON input. Executable and argv are fixed by the operator.
- **Independent checks.** A recipe with no check is rejected. Checks must inspect real state, not trust the planner.
- **Fail closed.** Failed/interrupted runs block replay until an explicit re-record. Corrupt state is an error, not a reset.
- **Exact invalidation.** Request, context, tool/registry/engine fingerprints are part of the match.
- **Bounded.** 1 MiB requests, 32 calls per recipe, 64 KiB per tool I/O, per-tool timeouts, process-group cleanup.
- **No rollback, no exactly-once.** A check can fail after an action ran. Inspect effects before re-recording.

Not a sandbox: tools run as the current user, and registry authorization is not a permission system. See the limits in [`README`](README.md#safety-and-limits) and the n8n notes.

## Interfaces

| Interface | Use it for |
|---|---|
| CLI | `run`, `record`, `approve`, `reject`, `undo`, `show`, `history`, `stats` |
| HTTP bridge (`replant serve`) | n8n or any language; bearer auth, `/run`, `/record`, `/inspect` (read-only), `/approve`, `/reject`, `/undo`, `/stats` |
| **MCP server** (`replant mcp`) | any MCP agent; 7 tools, stdio JSON-RPC |
| **n8n community node** | `n8n-nodes-replant/`, native operations + credential |
| **Python SDK** | `sdk/python/replant_client.py`, stdlib only |
| **TypeScript SDK** | `sdk/typescript/`, JS + `.d.ts`, no build step |
| **Adapters** | LangChain, CrewAI, AutoGen shims in `sdk/python/adapters/` |

MCP configuration:

```json
{"mcpServers":{"replant":{"command":"replant","args":["mcp","--store","/private/state","--tools","/operator/tools.json"]}}}
```

## Near-match review and undo

Exact match is the default. When an exact request misses, `run` proposes up to
three **near matches** (same context, token similarity ≥ 0.80) with
`status: review`. Nothing executes until a human or policy calls `approve`; a
`reject` suppresses that candidate for that request. Approve re-executes the
candidate recipe under the new request and only learns it if the checks pass.

Recipes may declare `undo` steps (registered as `kind: "undo"`). They run in
reverse when an action succeeds and a later check fails, and can be invoked
explicitly with `undo <record-id>`. Compensation is operator-declared, not a
transaction: a failed undo is reported, and replay stays blocked until a new
record.

## Roadmap

1. ~~MCP server~~ — shipped in v0.3.
2. ~~n8n community node~~ — shipped as a node package; not yet on npm.
3. ~~SDKs + framework adapters~~ — shipped as thin, tested clients/shims.
4. ~~Near-match review queue~~ — shipped with token similarity; embeddings are not used.
5. ~~Compensation/undo recipes~~ — shipped, operator-declared.
6. **Signed receipts + team policy sync** — the planned commercial layer.

## License

Apache-2.0 for the entire core. No open-source feature gates, no BSL.

The planned commercial layer is separate and hosted/team-oriented: policy sync,
signed receipts, SSO, audit retention, and a managed recipe registry. It is not
in this repository. That is the open-core split.

## Status

v0.3.0, experimental. Tested on Linux: 54 Rust tests plus Node/Python SDK and
node tests; real CLI/subprocess effects, restart persistence, concurrent writers,
corruption, timeouts, near-match approval/rejection, compensation, MCP stdio, and
a live n8n 2.46 record→replay loop. Not yet published to crates.io or npm.

If this saves you a model bill, star it and tell me which workflow you crystallized. If it breaks, open an issue with the receipt JSON — the journal is designed to make that debuggable.
