# Changelog

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
