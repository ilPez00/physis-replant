# Verified replay implementation plan

> **For agentic workers:** use executing-plans, task by task, with regression gates.

**Goal:** replace the misleading in-memory demo with persistent verified action replay.
**Architecture:** trusted tool registry, exact request identity, serialized local
store, JSON CLI for n8n's existing AI-miss branch.
**Tech stack:** Rust stdlib, existing serde/sha2/anyhow; libc for bounded Unix
process-group cleanup. No engine dependency or provider SDK.
**Spec:** ../specs/2026-11-04-verified-replay-design.md

## Global constraints

- Standalone, no physis-next dependency; no publication, credentials or billing.
- A cache hit executes actions and verification, not a cached side-effect result.
- State corruption and failed/interrupted verification must never authorize reuse.
- Store prompts/traces locally with private permissions; no telemetry.
- No synthetic latency or correctness advertised as measured performance.

## Review focus

- Same prompt with changed JSON input/context must miss.
- One learned recipe cannot authorize a different request.
- Tool failure/timeouts must invalidate reuse and leave a terminal receipt.
- A process restart must retain recipes and record/replay decision history.
- Concurrent writers and corrupt state must not silently lose evidence.

## Tasks

1. [x] Add failing CLI regressions for real action repetition, persistent replay,
   drift, failures, unsupported commands, output/timeout bounds and statistics.
   Run `cargo test --test replay_cli`; verify failures before implementation.
2. [x] Implement `workflow.rs` (request/recipe/registry/receipt types and Memo),
   `runner.rs` (registered JSON tool execution), `store.rs` (locking, journal,
   atomic private writes). Replace placeholder Memo; preserve extracted public
   cognition types and strengthen invalid-confidence/unoffered-action guards.
3. [x] Implement real `run`, `record`, `show`, `history`, `stats`, `--help` CLI.
   JSON stdin/stdout; miss is a routable result, failures nonzero. Gate tests.
4. [x] Replace performance claims with a real process-boundary benchmark and n8n
   example; correct canonical BSL text, preserve Apache provenance, document
   current limitations and migration from placeholder API.
5. [x] Run full suite, independent fault probes, benchmark, formatting and clippy
   if available; package verification. Review diff and keep changes local.

## Outcome

45 tests passed (25 unit, 18 CLI regressions, 2 process-loop tests). Formatting,
release build, package verification and replay-disabling mutation check completed.
Clippy and cargo-audit are unavailable on this host; live n8n/provider execution
was not performed. See ../../verification-2026-11-04.md for evidence and limits.
