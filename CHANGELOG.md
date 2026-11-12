# Changelog

## Unreleased

### Added
- Explicit `rerecord` in the library, CLI, HTTP bridge, MCP, SDKs and n8n node:
  re-execute a current indexed verified recipe with current tools and checks.
- Operator-owned `timeout_ms` values up to 1,800,000 ms (30 minutes), retaining
  the 30-second default, bounded I/O and process-group cleanup.
- Registry `kinds` role sets as an alternative to legacy `kind`, with explicit
  per-phase authorization and no implicit action permission.
- Opt-in recipe `bind_inputs: true` and typed `$input` key/JSON Pointer bindings
  across actions, preconditions, checks and undo; complete preflight before effects.
- n8n Request Timeout setting for explicitly authorized long recipes.

### Changed
- Exact fingerprint drift returns `stale` with the stored recipe/id, not an
  ordinary `miss`. This does not execute tools or automatically retry.
- Stats include a `stale` lookup count; successful re-records count as verified records.
- Rust callers constructing public structs must supply the new fields (see
  `docs/ops-recipes.md`); legacy registry/recipe JSON remains supported.

### Fixed
- Failed tool receipts retain bounded stdout, a stderr tail, parsed failure JSON
  where available, and actual exit codes. Truncation never becomes verification.
- HTTP/SDK/n8n inspection is read-only via `POST /inspect`; missing records
  return null (n8n wraps the result as `{record: ...}`).
- n8n processes every input item with its own payload and preserves item links.
- Near-match proposals and approvals require current fingerprints, an active
  index, a successful verification receipt, and no later failed/interrupted or
  undone execution. Failed and superseded records cannot be reused as candidates.

## 0.3.0 — Apache-2.0 open core

### Added
- **MCP stdio server** (`replant mcp`): 7 tools over JSON-RPC 2.0.
- **Near-match review queue**: exact misses propose up to three candidates with
  the same context and token similarity ≥ 0.80; nothing executes until
  `approve`. `reject` suppresses a candidate for that request.
- **Compensation and explicit undo**: recipes may declare `undo` tools; they run
  in reverse after a failed check, or on `undo <record-id>`. Replay stays blocked
  after an undo until a new record.
- **n8n community node** (`n8n-nodes-replant/`): run, record, inspect, stats,
  approve, reject, undo; credential with health check.
- **Python SDK** (stdlib) and **TypeScript SDK** (JS + `.d.ts`, no build step).
- **LangChain, CrewAI and AutoGen adapters** (optional, lazy imports).
- HTTP bridge routes for `/approve`, `/reject`, `/undo`, `/stats`.

### Changed
- **Relicensed the core to Apache-2.0.** v0.1.x and v0.2.0 remain BSL-1.1; this
  release and later are Apache-2.0.
- `BTreeSet`-based token similarity for review; no embeddings, no semantic
  model, no auto-replay.
- Receipts gain `candidates`; stats gain review/reject/undo counters.
- Test suite: 54 Rust tests plus Node and Python SDK/node tests.

### Safety
- Review approval re-executes actions and checks; it does not trust the candidate.
- Compensation is best-effort and operator-declared, not a transaction.
- Cross-context approval is refused; policy/engine fingerprints still apply.
