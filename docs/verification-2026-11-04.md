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
