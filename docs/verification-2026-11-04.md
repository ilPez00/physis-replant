# Verified replay: local evidence, 2026-11-04

Directory: `/home/gio/dev/physis-replant`, branch `fix/verified-replay`.
v0.2.0 remains local and unreleased. No GitHub/crates.io publication occurred.

## Gates

- `cargo test --offline --locked`: **47 passed, 0 failed, 0 ignored**
  (27 unit incl. 2 HTTP bridge tests, 18 CLI regressions, 2 process-loop tests).
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- `cargo build --offline --locked --release`: passed.
- `cargo package --offline --locked --allow-dirty`: packaged 29 files and
  verified compilation. This is a local package, not a published release.
- Fixed BSL terms compared with SPDX's canonical BUSL-1.1 text: identical after
  whitespace/quote normalization. This is a text check, not legal advice or
  verification of copyright ownership/commercial enforceability.
- `cargo clippy` and `cargo audit`: **unavailable**, commands not installed.
  CI contains the clippy gate; no claim that it passed locally.

## Real process loop, local planner fixture

Command:

```sh
python3 scripts/benchmark.py --binary target/release/replant --iterations 10
```

The final release-binary run observed:

| Arm | Planner calls | Replay hits | Actual increments | Independent checks | Total execution wall time |
| --- | ---: | ---: | ---: | ---: | ---: |
| Baseline | 10 | 0 | 10 | 10 | 980.19 ms |
| Replant | 1 | 9 | 10 | 10 | 974.30 ms |

Both arms independently verified 10/10 resulting increments. This fixture
avoided nine **local planner invocations**, not nine measured provider requests.
Its planner performs trivial local planning with no artificial delay. The small
timing difference is not evidence of an inference speedup. No token or dollar
saving is claimed. This is an n8n-compatible process boundary, not a live n8n
runtime benchmark. Raw final output for this session:
`/tmp/opencode/replant-verified-benchmark-20261104-final.json`.

## Does the regression suite discriminate?

In `/tmp/opencode/replant-verified-mutation`, replaced actual replay execution
with a fabricated successful receipt while leaving the repository untouched.
`records_then_replays_real_actions_across_processes` failed with:

```text
must re-execute action, not return old response
left: 1
right: 2
```

Unlike the original synthetic benchmark, the new test detects removal of the
feature it measures. No production source was left mutated.

## Independent review

Review identified and reproduced three defects during implementation, all fixed
with failing-then-passing regression tests:

1. Successful actions plus an index-publication failure could look like a clean
   miss. Any prior execution without its index now blocks automatic fallback.
2. Mid-run implementation drift could receive an old-policy success receipt.
   Recheck fingerprints before every tool and before terminal success.
3. Content-hash deduplication lost declared-path/symlink target associations.
   Keep the bindings in the policy hash while deduplicating content reads only.

Final independent review response, verbatim (same directory as above):

```text
Alias-swap finding fixed.

- Targeted regression: **1 passed**.
- Independent reproduction: swapping targets changes policy hash; `run` returns `miss`, zero tool calls.

No broader review or source edits.
```

Earlier implementation delegate response, verbatim, before subsequent hardening
and the final 45-test gate (same directory):

```text
Implemented:
- `src/main.rs`: strict JSON CLI, bounded stdin, proper exit codes.
- `tests/effectiveness.rs`: real subprocess/side-effect tests.
- `scripts/benchmark.py`, `examples/fixture_tools.py`: measured baseline/replay fixture.
- `examples/n8n/README.md`: fixed-command, base64 transport instructions.

Verification:
- Requested Cargo gate: **10/10 passed**.
- Benchmark `--help`: passed.
- Five-iteration benchmark: planner calls **5 versus 1**; both arms verified five increments/checks. No speedup claimed.
- No commits or branch changes.

Not checked: live n8n/provider; clippy unavailable; Physis checklist incomplete.
Risk: real actions have no rollback or exactly-once guarantee.
```

## n8n runtime (self-hosted container, 2026-11-04)

n8n 2.46.5 is a Docker container on `pansa`; the bridge is a user systemd
service on `172.17.0.1:8799`. The container could not reach any host interface
(tested `127.0.0.1:5678` self = 200; `172.17.0.1`, LAN and Tailscale bridge URLs
hung), so a temporary Cloudflare quick tunnel exposed the bridge for the runtime
test. This is a demo workaround: the URL is ephemeral and public, and the bridge
still requires its bearer token.

Imported via the public API, published and activated. Two production webhook
calls to the workflow produced:

| call | HTTP | receipt status | operation | provider_calls | observed counter |
| --- | ---: | --- | --- | ---: | ---: |
| 1 | 200 | succeeded | record | 0 | 3 → 4 |
| 2 | 200 | succeeded | replay | 0 | 4 → 5 |

Both calls re-executed the real action and independent check. The workflow used
literal request/recipe bodies (the template's placeholder planner, not an LLM).
This proves the n8n-compatible loop; it is **not** a live provider/token-saving
measurement. The n8n API key and bridge token are runtime credentials and are
not stored in this repository. The imported workflow and the running n8n
database still hold the bridge token; treat them as private local state.

## Claim matrix through n8n

`scripts/n8n_claims_test.py` drives three live n8n workflows: the loop
(`run` → `record` on miss), run-only, and record-only. It asserts the claims
against the real counter and the receipt journal, not against the bridge's
self-report. Two runs (different timestamped contexts) produced:

| claim | observed |
| --- | --- |
| one planner call, four replays, five actions | `record, replay, replay, replay, replay`; counter +5 |
| recipe survives bridge restart | next call `replay`; counter +1 |
| changed context | `miss`; counter unchanged |
| changed input | `miss`; counter unchanged |
| changed tool implementation | `miss`; counter unchanged |
| restored tool implementation | `replay`; counter +1 |
| failed check | `failed`; action had already incremented (no rollback) |
| next call after failure | `blocked`; no action, still `blocked` after clearing the marker |
| explicit re-record | `record` succeeded; next call `replay`; counter +1 each |
| stats | measured records/replays/failures/blocked/pending, `pending: 0` |

The first run's stats: 4 records, 11 replays, 2 failures, 2 blocked, 0 pending.
The second run: 6 records, 18 replays, 3 failures, 4 blocked, 0 pending. Deltas
match the operations the script performed. This is still a local fixture, not a
live model; it proves the loop behavior and invalidation rules, not token or
provider savings.

## Real LLM token measurement (crystallization)

`scripts/llm_cost_benchmark.py` compares two arms over the same repeated task:

- **Baseline:** every iteration calls the LLM, then executes/verifies the plan.
- **Replant:** first iteration calls the LLM once; later iterations replay.

The LLM is the local `gemma4-e2b-heretic-Q4_K_M` gateway on omo (free), so the
token counts below are real API usage, not estimates. The dollar column uses an
**example** hosted price ($0.10/M input, $0.30/M output) and was not billed.

Five iterations, both arms performed five real increments and passed five checks:

| arm | LLM calls | prompt tokens | completion tokens | total tokens | wall time | example cost |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| baseline | 5 | 375 | 1885 | 2260 | 18.37 s | $0.000603 |
| replant | 1 | 75 | 377 | 452 | 4.50 s | $0.000121 |
| saved | 4 | 300 | 1508 | **1808 (80.0%)** | 13.87 s | $0.000482 |

For N identical iterations the call count is N versus 1, so the token reduction
approaches `1 - 1/N` when each call uses the same prompt/recipe (N=5: 80%,
N=10: 90%, N=100: 99%). A changed request/context/tool implementation forces a
new LLM call, as the claim matrix above shows. This is the crystallization win:
repeated tasks stop paying for inference, not merely for cached prompt bytes.

Limits: the local model's completion tokens include its reasoning, so absolute
counts would differ on another model; prompt caching could reduce the baseline's
cached input price but does not eliminate its output tokens or calls. This is a
synthetic one-step task, not a production agent workflow.

## Remaining boundaries

- Docker access was denied; no live n8n installation, worker, workflow import or
  provider inference was tested. Wiring instructions are explicitly illustrative.
- Operator tools must enforce resource/tenant permissions. This is not a sandbox.
- Inputs/context and declared implementation files define invalidation. Unknown
  external state changes cannot be inferred; tools should remain immutable during
  invocations. Before/after fingerprints do not replace filesystem permissions.
- No rollback, automatic retries, exactly-once delivery or recursive self-modifying
  memorizer. Failures/crashes may leave partial effects or tool descendants.
- Requests/traces/tool outputs remain private local plaintext; secrets must stay
  out and operators must manage retention. Journal currently stops at 16 MiB.
