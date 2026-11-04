# physis-replant

**First call costs tokens. Every identical call after that is free — and you
can prove it with a receipt.**

Auditable cost crystallization for AI workloads. An operation runs once on a
model (T2), gets compiled into a rule, auto-validated on first successful
replay, and promoted to free replay (T0a) only after it earns traffic
(`min_replays` + `success_rate`). Every step leaves a receipt. Nothing is
fabricated; nothing auto-promotes without evidence.

## Install

```sh
cargo install physis-replant
```

## Usage

```sh
# Run a task (first call pays, later identical calls replay free)
replant run "restart k3s"

# Show crystallization stats
replant stats

# Demo: 10-iteration n8n loop with benchmark report
replant demo
```

## How it works

1. **First run (T2):** your model runs, produces a decision, cost is paid.
2. **Compile:** the decision trace is compiled into a candidate rule
   (quarantined, unvalidated).
3. **Auto-validate:** on first successful replay, the rule is auto-approved
   (no human gate).
4. **Promote:** after `min_replays` successful replays with `success_rate >=
   min_rate`, the rule promotes to T0a (free replay).
5. **Replay (T0a):** subsequent identical inputs hit the promoted rule —
   zero model cost, with a receipt proving what happened.

## License

Business Source License 1.1 — free for non-commercial and individual use.
Commercial production use requires a license from the author. Converts to
Apache 2.0 on 2030-11-04.

## Status

v0.1.0 — experimental. Crystallization correctness proven on synthetic tasks
(80% free runs after 2 warmup iterations, 5x speedup, 100% correctness).
Real-world task validation in progress.
