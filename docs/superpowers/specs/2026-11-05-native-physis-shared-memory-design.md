# Native Physis/Replant integration and shared memory

Status: approved for implementation by the operator's `implement` instruction.
Execution plan: `docs/superpowers/plans/2026-11-05-native-physis-shared-memory.md`.
Implementation is isolated on feature branches; default-branch integration is separate.

## Intent and approved scope

The operator requested native integration: Replant remains usable alone and is
also a component of physis-next. When asked about memory scope, the operator
selected **general notes and observations as well as execution memory**.

The goal is one implementation of recipe execution and one implementation of
shared observation memory, not a subprocess wrapper or two synchronised copies
that can disagree about whether an action is safe.

## Current evidence

- Replant owns its recipe registry, records, indexes and fail-closed receipt
  journal. `Memo` is a public Rust API. Its CLI, HTTP and MCP surfaces use it.
- physis-next has no current Replant Rust dependency. Its MCP catalogue is a
  thin projection over runtime/services; CLI can call the same tool handlers.
- Physis general memory already uses `Observation` and `ObservationSource`
  (`physis-kernel/src/observation.rs`) with JSONL storage, monotonic sequence
  numbers, cross-process locks and write-time redaction (`physis-store`).
- `Observation` contains `seq`, `at`, `source`, `subject`, `body`, optional
  duration/producer and default-empty `props`. Its old-row compatibility is
  already tested. `StorePaths` resolves the observation root and the existing
  `PHYSIS_STORE_DIR` override.
- The Physis checkout is dirty on `feat/activity-portal`, committed HEAD
  `f7f44a101`. Existing CLI/query/cognition edits and all untracked work must
  remain untouched. Replant is clean at `c71a8bf` before this document.
- Earlier `CRYSTALLIZE.md` and `REPLANT-PLAN.md` in physis-next are untracked
  historical sketches, not the implementation contract. Do not overwrite them.

## Approaches

1. **Shared Rust components, recommended and selected as the design proposal.**
   Extract the existing observation types and observation-log primitives into
   a small `physis-memory` crate maintained in the Replant repository. Both
   projects use it. Next also consumes the existing Replant recipe engine.
   This removes duplication without making standalone Replant load Next.
2. **Mirror independent memory implementations.** Less extraction initially,
   but divergent writers, schema drift and synchronization failure would
   become permanent concerns. Rejected for an authoritative shared store.
3. **Call a Next/Replant daemon or shell command.** Avoids Rust integration but
   makes native functionality depend on another running program. Rejected
   because the requested component must be native and independently usable.

The implementation choices below are proposals, not additional operator
requirements. Approval of this document accepts their boundaries.

## Dependency ownership

- `physis-replant/crates/physis-memory`: small standalone library for observation
  types, redaction and observation-log read/append/query primitives. No Physis
  runtime, models, networking, UI or recipe execution dependency.
- `physis-replant`: depends on `physis-memory`; owns recipe validation,
  registered-tool execution, persistence, compensation and replay policy.
- `physis-next`: depends on those same packages. Preserve the existing public
  `physis_kernel::Observation`/`ObservationSource` paths through re-exports.
  Preserve `physis-store` entry points as composition/path-resolution facades.
- Event/revision storage, ontology, graph folding and project/environment
  resolution stay in Next. Do not move the whole Physis store or kernel into
  Replant.
- Use pinned Git dependencies on a committed Replant revision initially; both
  package dependencies must resolve the same shared-memory crate revision.
  Do not publish crates or add sibling-checkout path dependencies as a hidden
  requirement. Replant must build from a fresh clone without Next installed.
- Do not consolidate every historical cognition module in this change. The
  recipe executor and shared memory become canonical; broader cognition
  extraction/refactoring is out of scope.

## Authoritative stores and compatibility

There are two different kinds of state, not two authorities for the same fact:

1. **General memory:** one observation JSONL log, shared by both tools. Notes
   written by either are immediately readable by the other; observations
   written by Next are readable through standalone Replant's memory API.
2. **Execution authority:** the existing Replant store/journal. Both native Next
   execution and standalone Replant call the same `Memo` against the same store.
   This alone decides whether a recipe is verified, reusable or blocked.

Keep existing observation JSONL field names, serde behavior, sequence ordering,
lock-file names and legacy-row handling. Preserve valid-prefix reads with an
explicit corruption/truncation indication and refusal to append after a torn
tail. Do not silently discard malformed data or weaken Replant's stricter
execution-journal validation.

Keep existing files in place: no bulk copy, history rewrite, automatic archive
or deletion. New standalone memory roots are private; do not silently chmod
existing shared workspace directories. Preserve existing public APIs where
possible and retain Apache-2.0 extraction notices.

## Project binding and standalone operation

- Memory is project-scoped, never a global union of home-directory stores.
- Next resolves its general-memory root through existing `StorePaths` and
  `PHYSIS_STORE_DIR` behavior. Its native Replant component uses that root.
- Standalone Replant gains an explicit memory-root option/environment override
  so it can open the exact same root. Without that setting it uses private
  local standalone memory; it never needs a Next daemon or binary.
- Execution-store selection remains separate from general-memory selection.
  Both callers can select the same existing `.replant` execution directory.
  Relative paths resolve against the selected project root, not an MCP
  server's unrelated working directory.
- Document paired commands showing both memory and execution roots. No hidden
  cross-project autodiscovery or automatic ingestion of sensitive directories.

## Native surfaces

- Native `physis replant` CLI operations call the library, not HTTP/subprocess
  wrappers. Include record, run, inspect, history, stats, approve, reject and
  undo with the same validations and receipts as standalone Replant.
- Native Physis MCP tools are thin calls into the same runtime integration.
  Inspect/history/stats are read-only; execution operations are classified as
  external effects; rejection/general-memory append are mutating operations.
  Permission metadata must not label action execution as read-only.
- Standalone Replant gains bounded general-memory remember/history/search
  surfaces through its Rust API and existing CLI/MCP/HTTP interfaces. Reuse
  common implementation across interfaces; no second memory search engine.
- Preserve existing Physis remember/history behavior and optional structured
  trace/transition payloads. Preserve existing Replant operation input/output
  contracts, including the read-only inspect behavior.

## Receipts in general memory

- Publish a redacted observation of each execution decision, including failures
  and pending/interrupted evidence. Store stable receipt references, operation,
  status, request/recipe/policy hashes and provenance. Do not automatically copy
  raw prompts, inputs, full cognition traces or tool stdout into broad context.
- Projection identity includes execution-store identity, receipt id and lifecycle
  status. A Started and a terminal receipt are different observations, while
  retries of the same projection are deduplicated under the memory-log lock.
- General-memory projections are searchable evidence, not a second authorization
  database. They can be reconstructed from the authoritative receipt journal.
- A memory-projection failure after an action must return the real receipt and
  an explicit synchronization warning, not pretend the action never happened.
  A read/sync operation can repair missing projections without executing tools.
- Use the existing conditional/batched append mechanisms. Never hold the
  execution-store lock while acquiring the memory-log lock, or vice versa.
  Snapshot receipt data first, release that lock, then project idempotently.
- Avoid adding an entire observation-log revision to every replay key: the
  component's own receipt/note writes must not invalidate its next replay.
  Preserve explicit request/input/context matching and caller-declared relevant
  context. Automatic semantic dependency inference is not included.

## Trust and privacy

- A remembered note, imported observation or claimed `status: succeeded` in
  general memory never creates/promotes a recipe or authorizes execution.
- Keep registry-owned executable/argv selection, independent checks,
  implementation fingerprints, failure blocking and explicit recovery.
- Reuse existing write-time redaction and expand tests to every newly exposed
  append path. Redaction is shape-based, not a guarantee of secret detection.
- Bound memory query results and body sizes; report truncation/corruption.
  Treat recalled text as data, never as instructions from the operator.
- No multi-tenant authorization, public memory server, cross-project sharing,
  semantic auto-approval or synchronization across machines is promised here.

## Verification and stop conditions

1. A fresh standalone Replant checkout builds/tests without any Next checkout
   or service. Existing CLI/HTTP/MCP and n8n recipe behavior remains covered.
2. Existing Physis observation fixtures deserialize identically and existing
   remember/history APIs preserve their results and optional payloads.
3. Append a note through Next; read it through standalone Replant, and reverse
   the direction. Read a Next-produced observation through Replant. No sync or
   copy step is required for general memory.
4. Concurrent writers across both binaries keep every observation with unique,
   ordered sequence numbers. Torn-tail, corruption, redaction and restart cases
   preserve existing fail-closed boundaries and expose degraded reads.
5. Record through either execution surface, replay through the other, and
   inspect through both without an action. Verify actual counter/check effects,
   not just response status. A failed execution remains blocked in both tools.
6. Shared-memory notes claiming successful execution cannot bypass verification.
   Context/tool drift continues to invalidate reuse, while ordinary memory
   append and receipt projection do not invalidate an otherwise exact recipe.
7. Repeated/concurrent receipt projection creates no duplicate lifecycle
   observations. Inject a projection failure after effects; repair projection
   without running another action. Restart between execution and projection.
8. CLI and MCP permissions/behavior agree. HTTP remains bearer-authenticated.
   Historical/native receipts are not labeled as current execution success.
9. Run relevant Replant and Next tests, formatting, available clippy and their
   full done gates on a suitable fleet node. Report unavailable/blocked gates
   explicitly. Re-measure any joint token-savings claim before publishing it;
   the existing n8n 99% figure remains evidence for standalone Replant only.

## Work isolation and release boundary

Use isolated worktrees. Do not reset, stage or incorporate the dirty Next
checkout's unrelated edits, untracked design sketches or evidence. Establish
the integration branch from an explicitly recorded committed base and agree
the merge target before touching its divergent/default branch history.

Keep the two repositories independently buildable throughout. If a new Replant
engine version invalidates old verified recipes, document the miss/re-record
behavior; do not rewrite safety fingerprints to conceal incompatibility.
Publishing packages, upgrading live n8n services and moving existing private
stores are not part of this implementation request.
