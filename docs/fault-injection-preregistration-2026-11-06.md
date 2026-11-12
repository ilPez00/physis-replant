# Fault-injection study — pre-registration (2026-11-06)

Written before running. Hypothesis and acceptance criteria fixed here; nothing
below may be changed after the first run except to note a deviation and its
reason.

## Hypothesis

**Pre-registered: zero silent-wrong replays under injected tool drift.** When a
replay-eligible request's registered tool implementation changes behavior
(silent drift: same exit code, same `ok:true` shape, wrong observed state),
replant must not return `status:succeeded` without a verifiable independent
check that still passes. Replay-with-checks catches drift; replay-without-checks
does not.

Null hypothesis: replay-with-checks catches drift at a rate indistinguishable
from replay-without-checks.

## Design

| Factor | Levels |
|---|---|
| Replay mode | A: replant-with-checks (default pipeline), B: replant-without-checks (recipe's `checks` emptied at record time) |
| Tool drift | none, silent-drift (check passes but state wrong), loud-failure (check returns ok:false), tool-unavailable (program missing) |
| Request | exact match (should replay), near-match ≥ 0.80 (should surface as `review`, not replay) |

Tools: operator-owned `increment`/`verify`/... shell fixtures from
`examples/fixture_tools.py`. Drift injected by replacing the tool script bytes
between record and replay (changes the fingerprint file, so replant must
invalidate on tool-implementation change) or by monkeypatching behavior while
keeping bytes identical (silent drift that fingerprinting alone cannot catch;
only the independent check catches it).

## Procedure

1. Build release binary at fixed commit (record hash).
2. For each (mode, drift) cell: record one verified recipe on an exact-match
   request, inject drift, run `run` on the same request 50 times.
3. For near-match requests: record verified recipe, then `run` a ≥0.80 similar
   request 20 times; expect `status:review` every time, never silent replay.
4. Independently observe real state (counter file + checks.jsonl) after every
   run; do not trust receipt fields alone.

## Acceptance criteria (pre-registered)

- Zero replays return `succeeded` under silent drift where the observed state
  contradicts the recipe's intended effect. A silent-wrong replay is a failure
  of the cell.
- Under loud failure or missing tool, replay returns `failed`/`blocked`, never
  `succeeded`.
- Near-match requests: 100% `review`, 0% silent `replay` without explicit
  `approve`.
- Replay-with-checks (A) detects silent drift in 100% of cells; replay-
  without-checks (B) detects 0% of silent-drift cells (expected, by design).
- Precision/recall at 0.80: measured on a labeled trace (see
  `scripts/trace_replay_benchmark.py`); reported with confidence intervals,
  not asserted.

## What is explicitly not claimed

- No rollback, no exactly-once (per README safety model). A check can fail
  after an action ran; effects are inspected before re-recording.
- This does not measure semantic generalization or savings for changing inputs.
- Token totals are local fixture counts; no paid invoice saving is asserted.

## Deviation log

(empty — to be filled only if a step is changed after the first run)
