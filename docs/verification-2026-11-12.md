# Issue 1 verification — 2026-11-12

## Findings and corrections

The follow-up audit found three defects rather than accepting the earlier green
summary at face value:

- Executable MCP emitted a non-JSON-RPC shutdown trailer and CLI setup errors on
  protocol stdout. New executable regression tests failed before repair. MCP now
  owns stdout through EOF, and setup errors use stderr. Acceptance rejects extra,
  duplicate, missing and non-JSON-RPC response frames; it no longer filters them.
- Explicit undo incorrectly required retired forward-action/check tools. A
  changed-registry regression failed before repair. Recovery now resolves only
  undo inputs against the recorded request and preflights every current undo
  permission before any recovery effect. Revoked later permissions prevent all
  earlier recovery effects too.
- Independent static review found binding expansion could allocate a huge tree
  before checking 64 KiB. Three process-level tests reproduced allocator aborts
  with a 64 MiB address-space cap and core dumps disabled, safely containing the
  reproduction. They now return ordinary bounded refusals, with no tool effects
  or journal. Coverage includes individually oversized selections, cumulative
  repeated selections, and excessive recipe calls. JSON bytes are budgeted
  before cloning; call count is checked before expansion.

## Final executable checks

Final code commit: `826384efceb5a15211088056a686f81cbad94842`.
Rust work ran serially on `omo-ts`, using an isolated source directory and
external Cargo target directory; no production service was changed.

- `cargo fmt --all`: passed; formatted source copied back before committing.
- `cargo test --locked`: 81 passed, zero failed or ignored.
- `cargo clippy --all-targets --locked -- -D warnings`: passed.
- `cargo build --locked --release`: passed.
- Strict TypeScript SDK typecheck: passed.
- Final release acceptance: eight checks passed, exercising actual subprocess
  effects, stale/rerecord, failure diagnostics, binding preflight, authenticated
  HTTP, Python and JavaScript SDK calls, and strict executable MCP framing.

Unchanged scanner/consumer suites passed in the preceding full serial gate run:
30 scanner, 11 Python SDK, 7 JavaScript SDK and 13 n8n node tests; benchmark also
passed. Final Rust and acceptance runs were repeated after the binding repair.

A real long action completed in 121.42131497300579 seconds in the preceding full
run at `645b79858277e03a8891d52b14b9de7fa546f593`. It was not rerun after the
binding-only allocation repair; the timeout runner did not change.

## Evidence and limits

Raw evidence on omo:
`/home/gio/.hermes/cache/scratch/replant-issue1-review-20261112/evidence/`

Key files: `mcp-stdout-red.log`, `mcp-stdout-green.log`, `undo-roles-red.log`,
`undo-roles-green.log`, `binding-budget-red.log`, `final-rust-tests.log`,
`final-clippy.log`, `final-release.log`, `acceptance-final-budget/report.json`,
and `acceptance-645b7985/report.json`.

The independent review was static and requested changes; the parent addressed
both required findings and verified them with the tests above. This is not a
claim of a separate post-fix reviewer approval. n8n node tests do not constitute
a fresh live n8n runtime deployment. Native polling was not added: long jobs use
operator-owned registry deadlines. Per-recipe dependency fingerprints are not
implemented; the entire registry and its declared implementation files define
invalidation. No merge, release, publishing or production deployment was done.
