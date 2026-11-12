# Verified local replay

Goal: fulfill the approved audit recommendation: repeat real actions, not old
responses, without repeat inference; persist prompts, decision traces and the
memorizer's versioned record/replay decisions. Standalone Rust, no physis-next.

## Approaches

- Response memoization cannot repeat side effects; reject it for workflows.
- Arbitrary generated shell scripts require an unsafe execution boundary; reject.
- Record typed calls to operator-registered tools, then execute and verify them
  on every invocation. Use this bounded approach. The operator supplies trusted
  tool implementations; a model supplies only tool names and JSON inputs.

## Contract

`run` receives `{prompt,input,context}` on stdin. Exact request + implementation
and tool registry fingerprints identify a learned recipe. On miss return JSON
`status:miss` for n8n to route to its AI node. `record` receives
`{request,recipe,trace}`, executes actions and independently registered check
tools, and remembers only verified recipes. First successful recording enables
automatic replay; no artificial warmup or self-reported confidence-as-proof.

Recipes contain `steps` and nonempty `checks`, each `{tool,input}`. Registry
entries contain fixed absolute `program`, fixed `args`, `kind:action|check`,
optional `fingerprint_files`, and bounded `timeout_ms`. Tool input travels on
stdin, never through generated shell. Tools must exit zero and emit one JSON
object containing `ok:true`. Run the actions and checks again on each hit.

The registry is operator authorization, not a sandbox. Tool authors must validate
inputs and access only permitted resources. No untrusted generated code enters
the executable or argv fields. Hash executables and declared implementation
files; changes invalidate reuse. Inputs/context must include tenant, policy and
external dependency versions relevant to correctness; unknown changes cannot be
inferred by this engine. No fuzzy matching.

Private local storage holds immutable learned records, atomic indexes and an
append-only started/terminal receipt journal. OS file locking serializes writers.
Failures or interrupted runs block reuse of that request until explicitly
recorded again. Corrupt state is an error, never a silent reset. No automatic
retry, rollback, exactly-once claim, secret uploading, or recursive self-modifying
memorizer. The memorizer records its strategy/version and decisions as receipts.

## Proof and scope

Regression tests cover actual side effects, restart, wrong request, failed checks,
policy drift, unsupported tool calls, timeouts, concurrent writers and corruption.
Benchmark runs real tool callbacks and measures wall time, calls and correctness;
an explicitly labeled local planner fixture substitutes for AI. Include n8n
integration instructions/export. Do not call it a live-provider saving or an n8n
runtime measurement unless those systems actually ran.

Repair license text/attribution and truthful documentation; company-restricted
licensing is source-available, not OSI open source. No publication or billing.
