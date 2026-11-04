# physis-replant

**Learn a verified tool recipe once. Execute its actions again without repeat inference.**

Standalone Rust library and JSON CLI for local, self-hosted automation. It stores
the complete request, structured cognition trace and recipe, plus a versioned
journal of the memorizer's own record/replay decisions. A replay **executes the
actions and checks again**; it does not return an old side-effect response.

Status: experimental v0.2.0, Unix process execution (tested on Linux).
**Source-available**, not OSI open source. No dependency on physis-next, no model
SDK, provider calls, telemetry, scheduler or hosted service.

## Try it

```sh
cargo build --locked --release
python3 scripts/benchmark.py --binary target/release/replant --iterations 10
```

The benchmark creates harmless temporary counter/check tools. Baseline plans on
every iteration; replay plans only on the first miss. Both must actually perform
and independently verify every increment. It reports wall time and planner-call
counts, not an assumed speedup. **Its local planner fixture is not a live model,
and its process loop is not a live n8n runtime benchmark.**

Install from this checkout with `cargo install --locked --path .`. Nothing has
been published to crates.io or GitHub by this change.

## Workflow

1. `run`: look up an exact `{prompt,input,context}` request.
2. On `status:miss`, your existing AI/planner proposes a recipe and trace.
3. `record`: validate every tool call, execute actions and independently
   registered checks, and persist the successful recipe automatically.
4. The next `run` executes that recipe and its checks, with no planner call.
5. A changed request, tool implementation, registry or engine invalidates reuse.
   An execution/check failure or unresolved crash **blocks** automatic reuse.

The planner must **not execute the proposed actions before record**, because
record executes them itself. Replant does not intercept arbitrary existing AI
sessions or learn arbitrary programs from prose. A caller must supply a typed
recipe over an operator-owned tool registry. Cognition traces are retained as
structured JSON, including additional spans; caller-provided reasoning is not
treated as proof. The memorizer records its version, strategy, policy hash and
decisions; it does not recursively rewrite or optimize itself.

## JSON CLI

```sh
replant run    --store /private/state --tools /operator/tools.json < request.json
replant record --store /private/state --tools /operator/tools.json < record.json
replant show   --store /private/state < request.json
replant history --store /private/state
replant stats   --store /private/state
replant --help
```

One JSON value on stdin, JSON only on stdout. Exit 0: success, lookup miss or
help. Exit 1: invalid input, failed execution, blocked replay or corrupt state.
The default state directory is `.replant`. Read-only inspection needs no tools.

Request:

```json
{"prompt":"increment once","input":{"amount":1},"context":{"tenant":"demo","workflow":"counter-v1"}}
```

Record envelope (the request must be the original, not model-rewritten input):

```json
{
  "request":{"prompt":"increment once","input":{"amount":1},"context":{"tenant":"demo","workflow":"counter-v1"}},
  "recipe":{
    "steps":[{"tool":"increment","input":{"amount":1}}],
    "checks":[{"tool":"verify","input":{}}]
  },
  "trace":{"provider":"your external planner","spans":[{"selected":"increment"}]}
}
```

Registry entries, authored by the operator, bind names to **fixed** executable
paths and argv. A model may supply only a registered name and JSON input:

```json
{
  "increment":{
    "kind":"action","program":"/usr/bin/python3",
    "args":["/operator/counter.py","increment","/private/counter"],
    "fingerprint_files":["/operator/counter.py"],"timeout_ms":2000
  },
  "verify":{
    "kind":"check","program":"/usr/bin/python3",
    "args":["/operator/counter.py","verify","/private/counter"],
    "fingerprint_files":["/operator/counter.py"],"timeout_ms":2000
  }
}
```

Each tool receives its JSON input on stdin. It must exit zero and emit one JSON
object containing `"ok":true`. Check tools must inspect actual outcomes rather
than accepting a model's assertion. Optional recipe `preconditions` use check
tools and run **before** any action. See `examples/fixture_tools.py` for the
harmless counter fixture; it is not a production access-control implementation.

## n8n

n8n normally runs in Docker, so the workflow cannot execute a host binary. Use
the HTTP bridge (`replant serve`) and call it from an HTTP Request node:

```sh
replant serve --bind 172.17.0.1:8799 --store /private/state \
  --tools /operator/tools.json --token-file /private/token
```

`POST /run` and `POST /record` mirror the CLI; `GET /healthz` is unauthenticated.
Every other route requires `Authorization: Bearer <token>`. Status: `200`
succeeded/miss, `409` blocked, `422` failed. The bind must name one interface
(`0.0.0.0` refused). No TLS.

[Wiring, registry and import steps](examples/n8n/README.md). The generated
workflow embeds its bearer token so it imports without credential setup; replace
it with an n8n header credential for shared instances. Not an n8n Cloud
integration.

## Safety and limits

- Registry authorization is **not a sandbox**. Tools execute as the current OS
  user. Register only trusted, input-validating implementations with appropriate
  tenant/resource permissions; do not expose arbitrary executables or code to AI.
- Include relevant tenant, authorization, policy and external dependency versions
  in `context`; declare script/config files in `fingerprint_files`. Undeclared
  external state changes cannot be inferred. Use preconditions for mutable state.
- Storage is private plaintext (0700 directories, 0600 new files on Unix), not
  encryption. Requests, traces and tool outputs may contain sensitive data: keep
  secrets out, protect the volume, and set your own retention policy. No uploads.
- A global OS lock serializes one store. Use a local filesystem with reliable
  locking/fsync, not independent worker volumes or an unverified network mount.
- There is no rollback or exactly-once guarantee. A tool may change state before
  a later check fails. Do not retry or re-record until effects are inspected.
  Killing the CLI can leave tool descendants alive; tools must not daemonize or
  escape their process group. Pipe reads/writes share the configured deadline.
- Max 1 MiB request/record, 128 registered tools, 32 recipe calls, 64 KiB per tool
  input/stdout/stderr, timeout 1..120000 ms per tool. Journal stops at 16 MiB:
  archive the whole store explicitly rather than silently dropping evidence.
- Receipts prove what trusted tools reported and the engine observed; they are
  not signed attestations or formal proof. `provider_calls:0` describes Replant
  only, not an external n8n AI node or a tool that itself calls a provider.

## Tests and honest metrics

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Regressions exercise separate CLI processes, actual side effects, independent
checks, full trace retention, context/implementation drift, crashes, concurrent
writers, timeouts, malformed output and corrupt state. Statistics count completed
record/replay executions. `replay_share_percent` is verified replays divided by
all execution attempts, including failed/pending ones; lookups are excluded.
It is not a measured token, money or latency saving.

The old v0.1 "80% savings / 5x speedup / 100% correctness" benchmark was synthetic
accounting and is withdrawn as effectiveness evidence. Legacy cognition helpers
remain available, but substring rule selection is **not** the workflow runtime.

## License

New replay implementation: [Business Source License 1.1](LICENSE). Personal,
non-commercial production use and non-profit use are permitted without a fee.
Business production use requires a separate commercial license; non-production
evaluation remains permitted by BSL. Converts to Apache-2.0 on 2030-11-04 (or
the earlier change required by BSL). Commercial terms are not set up here.

Extracted Apache portions remain Apache-2.0: [NOTICE](NOTICE),
[LICENSE-APACHE](LICENSE-APACHE). Existing Apache permissions are not revoked.
Legal review of commercial terms and copyright ownership is still required
before a commercial release; this README is not legal advice.
