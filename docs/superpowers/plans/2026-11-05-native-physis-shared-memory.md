# Native Physis and shared memory implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Native execution requested; one independent final review. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Keep Replant standalone while making its recipe executor and a common observation log native components of Physis-next.

**Architecture:** Extract Next's observation types, redaction and observation-log IO into `physis-memory` in Replant. Keep Next's public type/store paths as re-exports and facades. A single Replant `Memo` owns execution authority; receipt projection into general memory happens after releasing execution locks, is idempotent, and cannot authorize replay.

**Tech Stack:** Rust; existing serde/chrono/anyhow/libc dependencies; pinned Git dependencies in Next; existing CLI/MCP/HTTP surfaces.

**Spec:** `docs/superpowers/specs/2026-11-05-native-physis-shared-memory-design.md`, approved by the operator's `implement` instruction on 2026-11-05.

## Global constraints

- Standalone Replant must build with no Next checkout, binary or service.
- Preserve existing observation JSONL shape, sequence and `observations.lock`; preserve Next public observation/store paths and optional structured note bodies.
- Preserve the valid prefix with explicit degraded reads; refuse append after corruption/torn tails; no history deletion/migration or chmod of existing stores.
- Notes/observations never authorize a recipe; only the Replant journal does.
- No simultaneous execution-store and observation-store locks. Projection failure after effects returns the real receipt with an explicit warning; sync repairs without actions.
- CLI/MCP/HTTP use the same library implementation. Inspect/history/stats stay read-only; execution tools carry external-effect permissions; memory writes are mutations. HTTP stays authenticated.
- Work only in isolated worktrees. Next base `64ae52fe4`; Replant base `fa6f4d1`. Leave the dirty Next checkout and all its unrelated work untouched. No live services or package publishing.
- Native worktrees are integration candidates, not authorization to merge into Next's divergent/default branches.

## Review focus

- A forged successful note must not promote a recipe or bypass a failed execution.
- Valid JSON without a final newline and unreadable/duplicate-sequence logs must not be mistaken for safe append targets.
- Concurrent receipt projection and ordinary notes must retain every row without duplicate lifecycle projections.
- Projection errors after actual side effects must preserve the real execution result and repair without repeating the effect.
- Separate project roots, ordinary memory writes and source/tool drift must not leak authority or accidentally invalidate exact replay.

### Task 1: Canonical observation memory and compatibility facades

**Files:** Replant `Cargo.toml`, `crates/physis-memory/{Cargo.toml,src/lib.rs,src/observation.rs,src/redact.rs,src/log.rs,tests/memory.rs}`; Next kernel/store Cargo manifests and observation/log/redact modules.

**Interfaces:** `physis_memory::{Observation, ObservationSource, Memory, MemoryNote, MemoryQuery, MemoryPage}`; `Memory::append`, `append_if`, `append_batch`, `append_unique`, `remember`, `query`; `load_observations(path) -> (Vec<Observation>, bool)`. Preserve `physis_kernel::observation::*` and `physis_store::{append_observation,append_observation_if,append_observations,load_observations}` through facades.

- [ ] Write failing tests for legacy JSON, props/producer preservation, single/batch/conditional ordering, redaction, bounded queries, private new roots, torn/corrupt/unreadable logs, and concurrent append.
- [ ] Extract existing types/redaction/log semantics; retain Next event/revision storage in Next. Add explicit fail-closed checks for unsafe append targets without dropping readable evidence.
- [ ] Run `cargo test -p physis-memory`; expected all pass. Run relevant Next kernel/store compatibility tests after pinning a committed Replant revision; expected no type or API regressions.
- [ ] Commit the independently usable shared crate and compatibility facades on their respective feature branches.

### Task 2: Standalone memory surfaces and receipt projection

**Files:** Replant `src/{memory,workflow,store,main,mcp,server,lib}.rs`, integration tests, README/usage documentation.

**Interfaces:** `Memo::with_memory_root`, `remember`, `memory_query`, `sync_memory`; optional `Receipt.memory_warning`; CLI `--memory-root` / `REPLANT_MEMORY_ROOT` and `remember`, `memory-history`, `memory-search`, `sync-memory`. MCP/HTTP equivalents call the same methods.

- [ ] Write failing tests: remember/history/search, redaction, restart, actual action/check counter, projection of Started + terminal rows once, concurrent sync, failure warning and repair, failed execution remains blocked despite forged notes, ordinary notes do not invalidate replay, inspect causes no action/projection writes.
- [ ] Implement projection by snapshotting validated journal history, releasing its lock, then conditionally appending redacted receipt references under the memory lock. Never project raw prompts/inputs/output.
- [ ] Add CLI/MCP/HTTP adapters and bounded validated inputs; preserve existing operation contracts and auth. Bump engine minor version and document explicit re-record on version drift.
- [ ] Run complete Replant Rust tests, clippy, fmt, existing SDK/n8n tests and scanner receipts. Expected green, no customer/model calls.
- [ ] Commit Replant implementation; make a pinned Git revision available to Next without publishing crates or merging Next.

### Task 3: Native Next surfaces and cross-binary proof

**Files:** Next workspace lock/manifests, kernel/store facades, runtime `replant.rs`, MCP tool definition/dispatch/access metadata, CLI command module, integration tests and docs.

**Interfaces:** Native `physis replant <run|record|inspect|history|stats|approve|reject|undo|sync-memory>` calls the same library. Explicit project, execution-store and registry selection; shared memory root resolves via existing `StorePaths`. `physis.replant.*` MCP tools delegate to the runtime service, without subprocess/HTTP wrappers.

- [ ] Write failing tests for native tool catalogue/access, read-only inspect/history, library identity, cross-surface record/replay, bidirectional general notes, real Next-produced observations, different-project isolation, and projection-failure recovery without action.
- [ ] Add pinned dependencies on the same Replant Git revision; preserve all existing remember/history behavior. Resolve relative execution/registry paths against the selected project, not the MCP server working directory.
- [ ] Build both actual CLIs and prove Next record to Replant replay and vice versa against a disposable counter fixture; inspect must leave counter unchanged; notes must round-trip across both binaries.
- [ ] Run affected Next tests, full feasible done gates, formatting and clippy on a suitable node. Label unavailable gates and baseline failures, never weaken tests.
- [ ] Independent whole-change review across both worktrees; reproduce/fix Important findings RED→GREEN. Commit explicit files and report branch/commit/proof paths. Do not merge into the other agent's checkout.
