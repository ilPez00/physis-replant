# General memory and execution authority

Replant 0.4 adds `physis-memory`, a standalone Rust crate containing the shared
Physis observation schema, redaction and locked observation log. No Next runtime,
model, daemon or graph is needed. `default-features = false` exposes types only.

## Standalone use

```sh
cargo build --release
export REPLANT_MEMORY_ROOT="$HOME/project-memory"
printf '%s\n' '{"subject":"deployment","text":"Staging check passed","actor":"operator"}' \
  | target/release/replant remember --store "$HOME/project-execution"
target/release/replant memory-history --store "$HOME/project-execution"
printf '%s\n' '{"query":"deployment","limit":20}' \
  | target/release/replant memory-search --store "$HOME/project-execution"
```

`--memory-root DIR` overrides the environment. Without either, general memory
is private local `STORE/memory`. Execution-store selection remains separate.
New memory roots/files use private permissions; existing shared roots are not
chmodded. Inputs are at most 1 MiB; memory query limits are 1–1000 observations
(default 20), and returned observation content is capped at 1 MiB. History is
the newest matching rows, in sequence order. Queries are literal, case-insensitive
subject/body matches, not semantic retrieval or certification.

`MemoryPage.degraded` exposes corruption, IO failures, unsafe sequence order or
an unterminated tail; `truncated` exposes incomplete query output. Valid-prefix
evidence remains readable. Appending after damage is refused; no history is
discarded or automatically repaired. Shape-based redaction runs on all appended
observations; it is not a universal secret detector.

## Execution receipts are authoritative; notes are not

The original Replant journal still decides whether actions can run. A note saying
`succeeded`, even one copied from a receipt, cannot create/promote a recipe,
override a failed execution or grant permission to execute.

After each execution decision, Replant projects validated journal rows into
general memory. Projections contain store/receipt identity, lifecycle status,
operation and policy/request/recipe hashes—not prompts, tool input/output or
planner traces. Started and terminal rows remain distinct. Source + subject
deduplication happens atomically under the observation lock. No observation
revision is added to the replay key: ordinary notes do not invalidate recipes.

Execution locks are released before memory locks are taken. If projection fails
after effects, the real receipt is returned with `memory_warning`; do not retry
the action to fix memory. Repair the memory path/permissions/corruption under
operator control, then run:

```sh
target/release/replant sync-memory \
  --store "$HOME/project-execution" --memory-root "$HOME/project-memory"
```

Sync only projects existing journal evidence. It never runs tools. It also
projects a Started row left by an interrupted process; it does not turn that
interrupted execution into a success. Read-only inspect/history/stats and general
memory reads do not project or run actions.

## MCP and authenticated HTTP

MCP adds `replant_remember`, `replant_memory_history`, `replant_memory_search`,
`replant_sync_memory` and read-only `replant_history`. Existing seven recipe tools
remain. Memory writes/sync are not labeled read-only; execution tools declare
external effects. The CLI's `mcp --memory-root` configures the same library.

HTTP adds bearer-authenticated `POST /memory/remember`, `GET /memory/history`,
`POST /memory/search`, `POST /memory/sync` (body `{}`), and `GET /history`.
`serve --memory-root` selects the same log. No public/multi-tenant memory server
or automatic sharing across projects/machines is provided.

## Compatibility

Observation JSONL fields and `observations.lock` match Physis-next. Optional
`props`, producer and duration fields retain their old serialization behavior.
The shared crate owns observation IO; Next retains graph/event/revision logic.

Replant's engine version changes from 0.3 to 0.4. Old verified recipes miss due
to engine drift and require deliberate re-recording after checking their effects;
no safety fingerprint, journal or historical receipt is rewritten.

Native Next wiring lives on the separate `feat/native-replant-memory` integration
branch. Its kernel/store facades and CLI/MCP call these same libraries; Next
retains its graph/event/revision logic. `scripts/native_shared_memory_test.py`
accepts `--replant`, `--physis` and a new disposable `--out` directory. The actual
cross-binary proof verified seven fixture actions and seven independent checks,
both record/replay directions, bidirectional notes and Next-produced observations,
failure blocking, concurrent appends, drift invalidation and projection recovery.
Inspect and sync added no actions. The existing n8n token benchmark remains
standalone evidence, not a measurement of native integration or shared-memory
performance. No default Next branch is merged by this implementation.
