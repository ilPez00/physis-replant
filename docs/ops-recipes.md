# Ops recipes: diagnostics, deadlines and explicit reuse

This is the unreleased issue #1 extension. The fixed contract and implementation
plan are in `docs/superpowers/`; measured execution belongs in the dated
verification report, not in this guide. No release/deployment is implied.

## 1. Keep evidence when a tool fails

Each tool outcome records `ok`, actual `exit_code` when the process exited
normally, parsed `output` when available, and `error`. It also retains:

- `stdout`: at most the first 65,536 bytes of process stdout;
- `stderr`: at most the final 8,192 bytes of process stderr;
- `stdout_truncated` / `stderr_truncated`: explicit loss indicators.

Nonzero exit, timeout, malformed/non-object stdout JSON, `ok:false`, oversized
verification output and I/O/spawn errors remain failures. A JSON object such as
`{"ok":false,"error":"seed did not finish"}` is retained rather than replaced
with null just because its process exited with code 1. Truncated stdout is not
accepted as verification. An exit caused by a signal has no normal integer exit
code; inspect the error and captured streams.

The store and receipts are private local state. Raw tool output can contain
credentials, personal data or customer material. Review/redact an exported
receipt before sharing it; neither truncation nor JSON parsing is redaction.

## 2. Allow a particular tool to run longer

The operator owns the tool registry. Its existing `timeout_ms` field accepts
1..=1,800,000 milliseconds (30 minutes), with a 30,000 ms default. Selecting an
explicit longer value is the opt-in; a recipe/model cannot change it.

```json
{
  "seed": {
    "kind": "action",
    "program": "/usr/bin/python3",
    "args": ["/operator/tools/seed.py"],
    "fingerprint_files": ["/operator/tools/seed.py"],
    "timeout_ms": 1800000
  }
}
```

A recipe still needs independent check tools and may contain at most 32 declared
calls across all phases. This does not add polling, a detached-job scheduler or
an asynchronous server API. Each invocation remains bounded; process-group
cleanup still applies. Tools that deliberately escape their process group are
not contained by Replant: this runner is not a sandbox.

Long recipes hold the synchronous store lock and HTTP worker. Set the HTTP/SDK
client deadline above the whole recipe duration, not just one tool. Python:
`ReplantClient(url, token, timeout=1860)`; JavaScript:
`new ReplantClient(url, token, {timeoutMs: 1860000})`. n8n exposes
**Request Timeout (Milliseconds)**. Those examples allow one 30-minute tool plus
overhead; several long tools require a larger total client budget.

A disconnected/timed-out client has not established whether an effect applied
or whether server execution stopped. Inspect the journal and real state before
retrying. Clients do not retry automatically.

## 3. Distinguish stale from absent; re-record explicitly

`run` returns a nonexecuting lookup result when an exact request has an indexed,
previously verified recipe but its tool/registry/engine fingerprint changed:

```json
{
  "operation": "lookup",
  "status": "stale",
  "stale_record": {
    "id": "<the original record id>",
    "recipe": {"steps": [], "checks": []}
  }
}
```

The empty arrays above illustrate the response shape only, not a valid recipe.
The real response contains the stored template. It is not permission to run it.
A genuine absent request remains `miss`; a valid near-match remains `review`.
Tampered/unverified records and unsafe execution histories do not become stale
candidates merely because the fingerprint differs.

After reviewing implementation changes and explicitly authorizing repeated
side effects, pass the actual `stale_record.id`:

```sh
printf '%s\n' '{"id":"<record-id>"}' \
  | replant rerecord --store /private/replant-state --tools /operator/tools.json
```

The command reuses the stored exact request/template, validates the current
registry and all inputs, persists intent, executes actions and independent
checks again, and indexes a new record only after success. It does not
reconstruct a plan or trust previous check results. The journal operation is
`rerecord`; stats count successful re-records as `verified_records` and expose a
separate `stale` lookup count.

Equivalent explicit entry points:

- HTTP: authenticated `POST /rerecord`, body `{"id":"<record-id>"}`;
- MCP: `replant_rerecord` with the same arguments;
- Python: `client.rerecord(record_id)`;
- JavaScript: `await client.rerecord(recordId)`;
- n8n: **Re-record (Repeat Effects)**.

The id must still be the active indexed verified record with a clean execution
history. Superseded ids, interrupted/failed runs, undo history, missing indexes
or corrupt evidence are refused/blocked. A failed new re-record blocks later
replay. Inspect actual effects before explicitly constructing a fresh `record`
for recovery. Never route every stale/failed/timeout response to re-recording.

### Limit avoidable invalidation

Tool fingerprints include declared `fingerprint_files`. Put each tool's
implementation in its own file and declare any shared library/config files
explicitly. Editing a monolithic file referenced by every tool invalidates all
those recipes. This is a scope-control tip, not permission to omit real
implementation dependencies. The engine currently fingerprints the whole
registry; registry changes can still invalidate otherwise unrelated recipes.

## 4. Register one executable for several roles

Old registries remain valid:

```json
{"rollback":{"kind":"undo","program":"/operator/bin/rollback"}}
```

To authorize the same registered tool for explicit actions and compensation:

```json
{
  "rollback": {
    "kinds": ["action", "undo"],
    "program": "/operator/bin/rollback",
    "timeout_ms": 240000
  }
}
```

Supply exactly one of `kind` or `kinds`. Role sets must be nonempty, unique and
contain only `action`, `check`, `undo`. Both forms together, empty/missing roles
and unknown roles are errors; an omitted role never silently grants action
permission. Every recipe phase is checked before any effects. The receipt's
`kind` is the role exercised by that call. Preconditions and independent checks
require explicit check authorization. Declaring multiple roles is an operator
capability grant, not a claim that one implementation independently verifies
itself.

## 5. Opt into request parameter bindings

Existing recipes keep literal inputs, even objects containing `$input`.
Parameter substitution is enabled only by `bind_inputs: true`:

```json
{
  "request": {
    "prompt": "seed the requested number of days and independently verify it",
    "input": {"days": 7, "snapshot": {"name": "before-test"}},
    "context": {"environment": "authorized-test-vm"}
  },
  "recipe": {
    "bind_inputs": true,
    "steps": [
      {"tool": "seed", "input": {"days": {"$input": "days"}, "snapshot": {"$input": "/snapshot/name"}}}
    ],
    "checks": [
      {"tool": "seed_check", "input": {"days": {"$input": "days"}}}
    ]
  },
  "trace": {"planner": "operator-reviewed ops routine"}
}
```

The operator must separately register `seed` as action and `seed_check` as
check. The engine sends the resolved call input as JSON on the fixed
executable's stdin; it never interpolates a shell command or argv.

A singleton `{"$input":"days"}` selects a top-level key from `Request.input`.
A selector starting with `/` is a JSON Pointer, including `~0` for `~` and `~1`
for `/`. Numbers, booleans, arrays, objects and present null retain their JSON
types; missing is an error, not null. Objects mixing `$input` with other keys,
empty/non-string selectors and invalid/missing paths are refused. Selected
request data is inserted as data, not recursively interpreted as a new template.
There are no environment, file, expression, default-value or tool-output bindings.

All phases (including later checks and compensation) resolve and validate
before the first tool executes. Each resolved input must fit the 64 KiB tool
input bound. The saved/hash-checked recipe remains the template. Replay still
requires the entire exact request/input/context. A changed input may produce a
near-match proposal but never automatic execution. Explicit approval resolves
against that newly approved request, not the old candidate's values. Failure
compensation and explicit undo use the corresponding recorded request input.

## Compatibility and source callers

Legacy registry JSON using `kind` and recipe JSON without `bind_inputs` remain
accepted. False `bind_inputs` is omitted on serialization to preserve historical
literal recipe hashes. Old tool outcomes deserialize without diagnostic fields.
Engine source changes intentionally invalidate previous execution fingerprints;
use the explicit stale review/re-record path, not a fingerprint bypass.

Rust callers constructing public structs must supply the new optional/empty
role fields, binding opt-in, optional stale payload and diagnostic fields where
applicable. The JSON contract is compatible, but Rust struct literal source
compatibility is not promised for this experimental unreleased extension.

## Verify the boundary rather than only the response

Run the complete repository gates and inspect the dated verification report.
The standalone `scripts/issue1_acceptance.py` runs real filesystem tool effects,
CLI lookups/replays, checked re-recording, request approval/undo, authenticated
HTTP with both SDKs, and MCP stdio in a new private output directory. Its optional
long-timeout proof performs one real action lasting longer than the old
120-second limit. No VM, live customer workflow or LLM is used.
