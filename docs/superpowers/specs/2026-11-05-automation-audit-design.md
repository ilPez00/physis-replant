# Read-only automation audit and prospect scan

Scope approved in conversation: the operator requested online prospects and an
automatable Replant scan/receipt script; when offered local exports, read-only
live n8n API, or public prospect URLs, selected **all of the above**.

## Smallest complete tool

One stdlib Python CLI in `scripts/scan_automation.py`, focused tests alongside
the existing scripts, and usage/prospect documentation. Reuse the existing
Replant CLI and its registered action/check machinery; no Rust changes,
dependency installs, background service, crawler, database, or Physis-next edits.

Input modes:
- `files`: explicitly supplied n8n workflow JSON (single workflow or exported
  list), with optional saved execution JSON/CLI execution output.
- `api`: one explicitly selected workflow on an operator-supplied n8n instance;
  GET workflow and bounded saved execution data only. API key comes from an
  environment variable and is never written to configuration or reports.
- `prospects`: fetch a bounded list of explicit public HTTPS URLs. Report
  page/source evidence and automation-related terms, not alleged buying intent.
  No search engine API, recursive crawl, automatic contact extraction/messaging,
  login, verification bypass or customer discovery through private accounts.

## Evidence contract

The report separates observed facts, static optimization candidates, unsupported
or missing telemetry, and unmeasured outcomes. Graph structure can identify a
model node in a loop; it cannot certify equivalent inputs, safe replay or ROI.
Missing token usage is unknown, never zero. Initially recognize actual
OpenAI-compatible HTTP model response usage; flag other provider telemetry as
unsupported rather than guessing. Avoid aggregating parent/child usage twice.

Reports never claim customer token/dollar savings, replay eligibility, or
workflow correctness from a read-only scan. A Replant receipt proves that this
scanner and its independent artifact/source checks ran; it is not a receipt for
executing the customer's automation. No LLM is used by the audit itself.

## Replant execution

Generate an operator-owned registry fixing Python and this script's worker argv.
The action performs the read-only collection/analysis and writes audit artifacts;
the check independently validates artifact structure, hashes, evidence labels,
source binding and a fresh per-invocation challenge. The challenge is runtime
state, not a change to the replay request, so an unchanged scan can genuinely
replay the scan action and checks rather than returning an old report.

Use one private, explicitly selected output directory, serialize scanner calls
there, and refuse unrelated existing output files. Store implementation/config
fingerprints and input hashes; changed files cannot inherit stale verification.
Keep receipts per execution id and a latest summary. Never auto-retry blocked or
failed execution or auto-approve a near match. Explicit recovery requires a
record option after inspection. Network pages are fetched again on each replay.

## Safety and boundaries

- Customer workflow exports are data: never evaluate expressions, import them
  into n8n, run their nodes, or execute their commands.
- API mode makes GET requests only; limits and pagination coverage are explicit.
  No writes, activation, credential export, workflow execution or billing calls.
- Public prospect mode rejects authenticated/query-bearing URLs and private
  addresses, limits body sizes/timeouts and rejects redirects rather than
  following unreviewed targets. Stop on access/rate-limit errors; no bypass.
- Exclude raw parameters, headers, prompts, tool outputs and API keys from
  reports and receipts. Redact credential-shaped text in human-facing labels;
  warn that human review is still required before sharing reports.
- Private scratch/config contains no API key. Input workflow exports may contain
  secrets, so do not copy them into public reports or commit private test output.
- New files are private by default. No destructive cleanup of user data.

## Acceptance

1. Static loop/model findings cite source/node references and remain candidates.
2. Genuine n8n execution exports yield the correct observed usage totals; missing
   or unsupported usage remains unknown/partial. Duplicate exports do not inflate
   totals. Paired before/after savings are not fabricated.
3. API fixture proves bounded GET-only collection, environment-only auth, and
   explicit errors for unauthorized, rate-limited, malformed or truncated data.
4. Public-fetch fixture proves body limits, no redirects/private targets, and
   correct evidence/unknown labels. Verify selected real public pages separately.
5. Run the actual Replant binary: first scan records, second identical scan
   re-executes via replay with a new challenge; inspect artifact and receipt
   hashes. A missing/invalid artifact or stale challenge fails checks.
6. Failures block automatic reruns; changed source bytes cannot masquerade as the
   previous scan; exported malicious commands are never executed. Check secret
   markers are absent from output.
7. Publish verified prospect sources, a restrained approach message, executable
   commands and honest distinctions between tested fixtures and live customer
   integrations. No outreach is sent by the agent.
