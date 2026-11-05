# Replay safety retest — 2026-11-05

Code under test: `5c91b5f` (fixes on top of v0.3.0).

## Reproduced and fixed

- Python/TypeScript/n8n inspection previously called `/run`, executing actions.
  It now calls authenticated `POST /inspect`, returning the stored record or
  null without executing tools or appending journal entries. n8n wraps this as
  `{record: ...}` so a missing record is valid n8n item JSON.
- n8n previously evaluated only item zero. It now evaluates each item's
  operation/payload, executes sequentially, and preserves `pairedItem` links.
- Review previously enumerated files saved before verification, including
  failed records. Both proposals and direct approvals now require current
  engine/strategy/tool fingerprints, a matching recipe hash, an active index,
  and a successful verification receipt. Later failures, interruptions, undo,
  supersession and index-publication inconsistencies prevent candidate reuse.

## Gates and regression evidence

- `cargo test --offline --locked`: **55 passed**, no ignored tests.
- `node --test n8n-nodes-replant/test/node.test.js sdk/typescript/test.mjs`:
  **12 passed**.
- From `sdk/python`, `python3 -m unittest -v test_client.py test_adapters.py`:
  **7 passed**.
- `cargo build --offline --locked --release --bin replant`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- New regression assertions failed before their respective fixes. Temporarily
  bypassing the approval guard made `unsafe_candidates_cannot_be_proposed_or_approved`
  fail again by executing a failed candidate. Restoring the guard restored green.
- The unsafe-candidate test covers ten cases, including a newer successful
  record whose index was not published over the old record.

## Real HTTP check

A temporary authenticated release bridge bound to a Tailscale interface was
exercised with the actual Python and TypeScript clients and the n8n node's
`execute` method using real HTTP requests. State was isolated from the deployed
bridge and removed after verification.

- Hit/miss inspections across all three clients: **zero actions, zero journal
  writes**; missing records remained null.
- Three n8n input items: **three linked outputs, three real replay actions**.
- Independent counter/check log: one initial record plus three replay effects
  and four successful checks, matching bridge statistics.

This exercises the node code and bridge, not an installed n8n runtime.

## Fresh live-model loop comparison

Model: local `gemma4-e2b-heretic-Q4_K_M` via an OpenAI-compatible API.
Binary: freshly built release from the code above. This is a CLI integration
test with a live model, **not** an n8n runtime benchmark.

| Observed metric | Planner every iteration | Replant |
|---|---:|---:|
| Iterations | 100 | 100 |
| Planner invocations / HTTP attempts / completed responses | 100 / 100 / 100 | 1 / 1 / 1 |
| Prompt tokens | 7,500 | 75 |
| Completion tokens | 37,700 | 377 |
| Total API-reported tokens | 45,200 | 452 |
| Real action count | 100 | 100 |
| Independent check count | 100 | 100 |
| Replays | 0 | 99 |
| Measured execution wall time (seconds) | 515.841 | 15.845 |

**99.0% fewer reported tokens**, avoiding 99 planning requests at equal observed
action/check counts. These are local model token counts, not paid invoice
savings. The provider reported cached prompt tokens; totals above are its full
reported usage, not a claim about uncached compute or billing.

Method: reused `scripts/benchmark.py`'s `run_arm` with its per-iteration counter
and check-log assertions, replacing only its local planner fixture with
`scripts/llm_cost_benchmark.py`'s real `llm_plan`. The audit wrapper counted each
HTTP attempt before sending it, disabled retries, required positive token
telemetry on every response, and required exactly one increment and one check
in the generated recipe. It did not trust a receipt's `provider_calls` to count
external inference. Both arms used private temporary state and the unchanged
2,000 ms fixture tool deadline. No deployed counter or recipe was touched.

This proves savings for an identical repeated task with lookup **before**
inference. It does not prove semantic generalization, savings for changing
inputs, or removal of model calls in an agent that still reasons every turn.

## Limits

Local `cargo clippy` is unavailable (`no such command: clippy`). No lint
suppression, dependency, skipped test or lowered timeout was added. The running
n8n/bridge deployment was not upgraded. Updated inspection clients require the
updated bridge; an older bridge returns 404, never an execution fallback.
