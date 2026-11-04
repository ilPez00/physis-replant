# Changelog

## 0.2.0 — unreleased

### Changed
- Replace in-memory response memoization with persistent verified tool recipes.
  This is a breaking API correction: use `Memo::open`, `record`, `run`, `inspect`,
  `history` and `stats` with `Request`/`RecordInput`; placeholder `MemoKey`,
  `MemoResult`, `CostReport`, `Tier` and no-op `flush` are removed.
- Implement real stdin/stdout JSON CLI commands instead of ignoring arguments.
- Add `replant serve`: bearer-authenticated HTTP bridge (`/run`, `/record`,
  `/healthz`) for Dockerized n8n; refuses unspecified bind addresses.
- Retain full structured traces and versioned memorizer decisions across restarts.
- Withdraw synthetic performance/correctness claims; add measured process tests.

### Safety
- Independent postchecks, optional preconditions, request-scoped recipes and
  executable/declared implementation fingerprints guard automatic reuse.
- Failed/interrupted runs block replay. Corruption is an error, not a reset.
- Bound process time, pipe memory and input; serialize store updates with OS locks.
- Restore canonical BSL covenants and preserve extracted Apache portions/notices.
  Source-available status is explicit; commercial legal review remains outstanding.
