# Issue #1: observable failures and reusable ops recipes

Scope authorized by the operator: evaluate and fix the diagnostic bug and implement the four feature requests in ilPez00/physis-replant#1. This is an implementation design, not a claim of completed work.

## Evaluation and source evidence

Base: origin/main, 64ed72b (not the local feat/automation-audit-plan branch). Current runner discards process status and captured streams whenever invoke returns Err; workflow therefore writes exit_code/output as null. Registry validation rejects timeout_ms >120000. Exact-entry fingerprint drift returns miss without its recipe. ToolSpec has one kind. ToolCall.input is passed literally; approve therefore reuses the old fixed values. Relevant owners are src/runner.rs, src/workflow.rs and src/store.rs; CLI/HTTP/MCP are thin projections.

Physis context retrieval was attempted before implementation; the plugin gateway rejected injected metadata (conversation_id/session_id/task_id/user_task) as undeclared arguments. No empty-index inference is made. Ordinary source inspection provides the evidence for this bounded repository. The full current engine at /home/gio/dev/physis-next on pansa is available for comparison, not a dependency to introduce. Retired physis-pro is out of scope.

## Capability map and order

| id | Responsibility | Depends on |
|---|---|---|
| diagnostics | Preserve bounded process evidence on every error path | runner |
| long-timeouts | Allow explicit operator-owned long tool deadlines | diagnostics |
| stale-recovery | Distinguish drift and explicitly re-execute a stored recipe | existing store/journal verification |
| registry-roles | Authorize each recipe phase through declared tool roles | registry validation |
| input-bindings | Bind templates under the exact request, including approve/undo | recipe preflight |
| projections | Expose the same contracts in CLI, HTTP, MCP, SDKs and n8n | five capabilities |

## Contract

1. Diagnostics: additive defaulted ToolOutcome fields hold bounded stdout (64 KiB prefix), stderr (8 KiB tail), and explicit truncation flags. Keep parsed JSON output when possible, including ok:false. A nonzero exit records its actual exit code. Timeout, invalid JSON, output overflow, spawn/I/O failure, signal exit and failed checks remain failures; truncated or partial output must never become verification. Drain streams without unbounded memory and retain available evidence even on timeout. Existing private-store permissions remain; receipts can contain sensitive tool output and must not be published automatically.

2. Long tools: timeout_ms remains operator-owned and defaults to 30000. Accept only 1..=1800000 (30 minutes); an explicit larger timeout is the opt-in, with no model-supplied timeout override. Preserve process-group termination, bounded pipes and 32 declared calls. Prefer this small registry extension over a new polling/daemon/job scheduler. Native polling is not added. HTTP and SDK timeouts remain explicit client choices, documented for long calls; a client timeout is unknown effects, never permission to retry.

3. Stale recovery: run returns status:stale, reason and a stale_record {id,recipe} for an exact indexed record whose registry/tool/engine fingerprint changed. This is a lookup only: no actions/checks. Validate the original record hash/schema/request and successful verification receipt before exposing it. An unverified/tampered/interrupted/failed/undone record stays refused/blocked, not a stale candidate. Add explicit rerecord with input {id}; it uses the stored exact request/template and current registry, preflights all phases, persists intent, reruns actions and independent checks, and changes the index only on success. Reject obsolete/non-indexed source ids and unsafe source histories. No automatic re-record/retry. Expose it in the library, CLI, HTTP and MCP. Journal/stats recognize successful re-records as verified records; count stale lookups separately. A failed rerecord must block later run.

4. Registry roles: accept the old kind:'action' form or a new kinds:['action','undo'] form. Require exactly one form, nonempty unique valid roles, and reject both/empty/unknown/duplicate roles. Never infer action permission from an omitted kind. Validate every phase before any side effect. Outcome.kind is the role actually exercised, not a tool's first role. Wire compatibility for old registries is required; note additive Rust struct fields/source compatibility in docs. No silent widening of roles.

5. Parametric recipes: opt in with bind_inputs:true on Recipe (omit the field when false, preserving old serialized recipes/hashes). Within every call input, a singleton {'$input':'days'} resolves a top-level request.input key; {'$input':'/nested/days'} resolves a JSON Pointer. Preserve the JSON value type. Refuse malformed, missing, ambiguous bindings and oversized resolved inputs before ANY tool call, including errors in a later check or compensation. When opt-in is absent, all input objects remain literal. Resolve all preconditions/actions/checks/undo together against the current exact Request before execution; save/hash the template, not resolved values. approve resolves using the newly approved request's input; replay remains keyed by the entire exact request/context. Explicit undo uses the recorded request input. No expressions, shell interpolation, environment/file lookup or inferred binding. Missing and null are distinct; present null is valid.

## Boundaries and architecture

Rust 1.89+, existing serde/serde_json/sha2/anyhow/libc, no new dependency. Keep process capture logic in runner; extract the binding resolver into a focused module if workflow grows. Fingerprint every execution-semantic module. Keep projections thin and update shipped SDK types, helper methods and n8n operation list. Do not alter unrelated branches, provider configuration, production services or external workflows. Do not post an issue response, close the issue, or publish a release. Use an isolated branch, focused verified commits and a draft PR after local gates.

## Feasibility, cost and risks

This is a bounded extension to the existing synchronous engine. Registry roles and resolved templates are security-sensitive: preflight is mandatory before effects. Adding status:stale is a new observable outcome; all shipped consumers must handle it without treating it as replay success or automatically recording. Engine source fingerprint changes intentionally invalidate prior recipes. Error output retention increases journal size but stays bounded per stream/call. Long calls hold the store lock and HTTP worker synchronously; no concurrency/async guarantee is added. Client disconnect/timeout cannot prove cancellation. Re-recording performs real effects again and is not exactly-once or rollback.

Build/test on omo, with isolated source and target directories and CARGO_BUILD_JOBS=2. Its measured available RAM is >8 GiB and no cargo process was active; root filesystem is already 89%, so avoid unrelated cache growth or cleanup and report storage limits. pansa is edit/review only. No large physis observe or new model load.

## Acceptance and verification

- Write failing regression tests before implementing each behavior; save red/green command receipts.
- Real subprocess tests for nonzero structured failure, stderr-tail retention, stdout overflow, malformed JSON, timeout with partial output, signals, closed stdin and unchanged successful execution.
- Registry deadline boundary tests, short real timeout cleanup, plus one real >120-second action under an explicit longer timeout outside the fast suite.
- Stale lookup executes zero tools; rerecord reruns actions/checks; failures/tampering/obsolete ids/failed source journals refuse or block as specified.
- Legacy and multi-role JSON tests, per-phase permission refusal before effects, and accurate role attribution.
- Binding tests for nested arrays/objects, pointers/escaping, numeric/null values, missing/malformed bindings, oversize resolved inputs, later-phase preflight refusal, replay/approve/explicit undo and failed-run compensation.
- Exercise actual CLI, authenticated HTTP and MCP projections plus Python/TypeScript SDK and n8n node surfaces.
- Gates: cargo fmt --all -- --check; cargo test --locked; cargo clippy --all-targets --locked -- -D warnings; cargo build --locked --release; real scanner unittest with REPLANT_BIN; scripts/benchmark.py; Python SDK unittest; node --test SDK and n8n tests.
- Fresh-context spec review, then correctness/security review. Repair required findings and rerun affected gates. Record exact tested commit and no skipped tests in docs/verification-2026-11-12.md.
- Update README/CHANGELOG/examples, report not-measured performance as such, verify remote branch/PR by readback. GitHub-hosted CI failure is not a green gate; classify billing/platform versus code from real annotations.
