# Walkthrough — stop paying an LLM to plan the same task twice

*A measured record→replay loop inside real n8n. Published 2026-11-21 as part of the
portfolio evidence audit. Numbers below come from the committed measurement
[`n8n-runtime-2026-11-05.md`](n8n-runtime-2026-11-05.md) and its
[machine-readable summary](n8n-runtime-2026-11-05.json); the unit suite was re-run green on
a fresh checkout on 2026-11-21 (25 passed, 0 failed).*

## The problem

An automation that repeats the same request pays for planning every single time. The model
is asked to produce the same recipe, over and over, at full token price — even though the
actions and checks are identical and already verified.

Caching the model's *response* is the usual shortcut, and it is wrong for anything with side
effects: a cached "I incremented the counter" does not increment anything.

## What Replant does instead

Record a **verified tool recipe** once — the real actions and the real checks, executed and
confirmed. On every repeat, replay the **actions and checks themselves**, with no inference
and no cached side-effect responses. The model may *name* trusted tools; it never chooses an
executable.

```
lookup(request) ── miss ──> planner (LLM) ──> record: execute + check ──> recipe stored
       │
       └──────── hit ──> replay: execute + check again, provider_calls: 0
```

## The measurement (one 100-item run per arm, real n8n)

Setup: n8n **2.46.5** CLI-mode execution (`import:workflow` + `execute --rawOutput`,
isolated SQLite), Replant v0.3.0 built from `b1827d4`, a local OpenAI-compatible model.
The LLM call is a **real n8n HTTP Request node inside the loop** — not a Python mock.
Baseline calls the model then records every iteration; Replant looks up first, and only a
`miss` reaches the model.

| Iterations | Baseline model calls | Replant model calls | Baseline tokens | Replant tokens | Saved |
|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 1 | 452 | 452 | 0.0% |
| 10 | 10 | 1 | 4,520 | 452 | 90.0% |
| 50 | 50 | 1 | 22,600 | 452 | 98.0% |
| **100** | **100** | **1** | **45,200** | **452** | **99.0%** |

These are cumulative checkpoints from **one** execution per arm, not six trials and not a
projection. Both arms completed **100 real actions and 100 successful independent checks**;
the check log was verified entry by entry against actual counter values, not against each
receipt's self-reported status. Wall time including CLI startup: **518.962 s baseline vs
16.339 s Replant**.

## What this is not

- Not a general savings claim: exact-repeat fixture, simple model and checks, one local run.
- Not a paid-invoice saving: the local provider reports prompt-cache tokens; totals include them.
- Not magic: a hand-written fixed workflow can avoid inference altogether. This measures
  bypassing repeated *planning* after a recipe is supplied once.
- Not a cache of side effects: every replay re-executes the action and re-runs the check.

## Try it

```bash
git clone https://github.com/ilPez00/physis-replant
cd physis-replant && cargo test --locked     # 25 tests, green on fresh checkout 2026-11-21
```

Full reproduction: [`scripts/n8n_runtime_benchmark.py`](../scripts/n8n_runtime_benchmark.py)
with the [n8n setup instructions](../examples/n8n/README.md#actual-n8n-loop-with-a-live-model).
Do not run Python with `-O`: assertions are verification gates.

Safety model, near-match review and undo: see the [README](../README.md).