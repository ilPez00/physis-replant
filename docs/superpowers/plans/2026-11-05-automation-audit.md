# Automation Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement task-by-task. Steps use checkbox syntax.

**Goal:** Scan local n8n exports, a selected live n8n workflow, or explicit public prospect pages and produce honest reports plus real Replant scan/check receipts.

**Architecture:** One read-only collector/analyzer CLI delegates fixed scan/check workers through the existing Replant binary. Artifact hashes and a fresh runtime challenge bind each receipt to the current scan; findings never become customer savings or execution authorization.

**Tech Stack:** Python standard library; existing Replant CLI; unittest HTTP fixtures.

**Spec:** `docs/superpowers/specs/2026-11-05-automation-audit-design.md`.

## Global Constraints

- All three modes approved; no changes or commands in physis-next.
- No new dependencies, model calls, customer workflow execution, API writes or outreach.
- Never publish raw workflow parameters, authorization values or prompts.
- Unsupported/missing telemetry is unknown; all static optimization findings are candidates.
- Network reads have explicit time/body/count limits; no redirect following or access-control bypass.

## Review Focus

- Duplicate/aggregate provider usage must not be double-counted.
- Exported commands and expression strings must remain inert data.
- Malformed/rate-limited API responses must not produce a successful audit.
- An old artifact must not satisfy the new scan's check.
- Private URLs, redirect targets and API keys must not leak into public prospect scans.

### Task 1: Analysis and read-only adapters

**Files:** Create `scripts/scan_automation.py`, `scripts/test_scan_automation.py`.

**Interfaces:** `analyze_workflows(workflows, executions) -> dict`; bounded JSON and public-page collectors; `main()` exposing files/api/prospects.

- [ ] Write failing tests for an LLM-in-loop export, no-loop export, inert malicious expression, real OpenAI usage, absent usage, unsupported telemetry and duplicate execution data.
- [ ] Run `python3 -m unittest discover -s scripts -p test_scan_automation.py -v`; confirm expected failures.
- [ ] Implement graph/evidence analysis and explicit schema handling, reusing existing CLI execution parsing where appropriate.
- [ ] Test GET-only n8n collection/auth/coverage and public-page bounds/redirect refusal using stdlib HTTP fixtures.
- [ ] Run focused tests; verify failure labels and secret exclusion.

### Task 2: Replant receipts and freshness

**Files:** Same CLI and tests.

**Interfaces:** fixed worker subcommands for scan and check; report JSON/Markdown; per-id Replant receipts and latest receipt summary in the private output root.

- [ ] Write failing integration tests using the actual built Replant executable: record, replay, new challenge, changed source, malformed artifact and failure blocking.
- [ ] Implement private output ownership/locking, config fingerprinting, trusted tool registry and record-on-miss only; preserve blocked/review behavior.
- [ ] Validate artifacts independently in the check worker; never accept an earlier invocation's challenge.
- [ ] Run focused tests and the real exported n8n fixture from the previous benchmark. Do not execute customer nodes.

### Task 3: Operator documentation and publish

**Files:** Create `docs/automation-audit.md`, `docs/prospects-2026-11-05.md`; link from README; add focused Python tests to existing CI if they are hermetic.

- [ ] Document each mode's command, API environment variable, receipt meaning, supported telemetry and limitations.
- [ ] Record verified public prospect sources and contact routes; distinguish potential partners from confirmed buyers. Include one short manual outreach draft.
- [ ] Check published examples and reports against actual scanner output; no fabricated savings or customer assertions.
- [ ] Run syntax checks, focused Python integration tests and nearest existing gates; label unavailable/live-untested paths explicitly.
- [ ] Review diff, stage explicit paths, commit/push only Replant after verification. Leave physis-next untouched.
