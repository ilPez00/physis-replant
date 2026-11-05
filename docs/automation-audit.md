# Read-only automation scans with Replant receipts

Scan n8n exports, one authorized n8n API workflow, or explicit public prospect
pages. No customer workflow is imported, executed, activated, or modified. No
model calls, new Python dependencies, crawling, or outreach.

Requires Python 3.10+ on Linux/macOS and the existing Replant executable:

```sh
cargo build --locked --release
export REPLANT_BIN="$PWD/target/release/replant"
python3 scripts/scan_automation.py --help
```

Run from this repository. Use a separate output directory per customer or
prospect batch; it must be new or scanner-owned and private (`0700`). The
scanner creates private files and refuses unrelated existing output files.
Do not run as root or on shared/untrusted writable storage.

## 1. Exported files (offline)

```sh
python3 scripts/scan_automation.py files \
  --workflow /path/to/workflow.json \
  --executions /path/to/executions.json \
  --out "$HOME/replant-audits/customer-a"
```

`--executions` is optional. Workflow input is one exported workflow, a workflow
array, or a `data` array. Execution input is one execution, an array, a `data`
array, or the saved CLI execution log format used by this repository's n8n
benchmark. Saved execution data needs `data.resultData.runData`.
Exports are inert data, including embedded commands and expressions.

## 2. Authorized live n8n API (GET only)

Load `N8N_API_KEY` securely into your environment before running; never put the
key in the URL, shell command, report, or repository. Use the narrowest available
read permissions on an instance you are authorized to inspect.

```sh
python3 scripts/scan_automation.py api \
  --instance-url https://your-instance.example \
  --workflow-id YOUR_WORKFLOW_ID \
  --limit 100 \
  --out "$HOME/replant-audits/customer-api"
```

`--key-env NAME` selects a different environment variable. The key is inherited
by the registered collector process, not written to the scanner configuration.
Exactly two GETs: `/api/v1/workflows/ID` and
`/api/v1/executions?workflowId=ID&includeData=true&limit=N`. One page, at most
100 requested executions. No pagination traversal: a `nextCursor` marks coverage
incomplete. Even a complete page means only the selected endpoint's returned
scope, not every historical execution or the entire account. n8n retention and
permissions may omit older data. Redirects, auth failures, rate limits, malformed
responses, and oversized bodies fail rather than retry silently.

Use HTTPS. HTTP is allowed only for explicitly selected non-public hosts, for
local/private operator environments; it does not encrypt credentials.
No customer API instance was used in verification: API integration was tested
against a local HTTP fixture with a fake credential.

## 3. Public prospect pages

```sh
python3 scripts/scan_automation.py prospects \
  --url https://www.makeitfuture.com/solutions/n8n-automation-services \
  --out "$HOME/replant-audits/public-prospects"
```

Supply 1–5 explicit public HTTPS URLs, repeating `--url` as needed. Each source
is fetched once per scan, with a 10-second socket timeout and 16 MiB body limit.
Only HTML/plain text; no authentication, query strings, custom ports, private
addresses, or redirects. DNS must resolve exclusively to public addresses; the
validated address is pinned for the TLS connection. Use canonical URLs yourself
if a site redirects. No bypass if a page blocks requests.

Report contains page title, source hash, and terms such as `n8n` or `workflow`.
These are relevance signals, not evidence of buying intent, spend, or a
technical optimization opportunity. Contact discovery and sending remain manual.
See [verified prospect routes](prospects-2026-11-05.md).

## Artifacts and receipt meaning

- `report.json`: source hashes, static candidates, measured/unknown telemetry,
  fresh challenge, configuration hash, and explicit outcome limitations.
- `report.md`: human-readable rendering of that same report.
- `report.sha256`: current JSON artifact checksum.
- `receipts/ID.json`: per-run Replant receipt bundle with artifact hash on success.
- `latest.json`: latest result; CLI also prints this bundle as JSON.
- Private `config.json`, `attempt.json`, `tools.json`, `state/`, `scan.lock`, and
  ownership marker: implementation details, not public deliverables.

First successful scan records the fixed scanner/check recipe. An unchanged
repeat replays it by **running collection and checks again**, with a new
challenge—not by returning an old report. Source/config/implementation changes
cannot silently reuse stale verification. Source bytes are hashed, not copied.
Old receipt bundles retain artifact hashes; reports themselves are latest-only.
Archive the reports beside the matching receipt if historical content is needed.

The check validates challenge/config binding, evidence hashes, token consistency,
unmeasured outcome labels, Markdown agreement, and current artifact checksum.
Files mode also verifies source bytes still match. These are local integrity
checks, not a signature, third-party certification, or protection against someone
who controls the scanner account and rewrites both evidence and checks.

**A receipt attests scanner execution and checks. It does not attest customer
workflow correctness, safe replay eligibility, customer execution, or savings.**
`customer_savings` remains `null`, `replay_eligibility` stays `unverified`.
Zero model calls by this deterministic scanner is not customer token savings.

Exit status is zero only for a successful receipt. Failed/blocked scans do not
reference a previous successful artifact as current evidence. Inspect
`latest.json` and the receipt errors before recovery; `--record` explicitly
re-records after that inspection. It is not automatic retry or near-match approval.

## Supported evidence and limits

- Cyclic regions containing a Loop Over Items node and model node produce
  **candidates**. They do not establish equivalent inputs or safe actions.
- Token accounting recognizes OpenAI-compatible HTTP responses with `choices`
  and nonnegative integer `prompt_tokens`, `completion_tokens`, `total_tokens`,
  where total equals prompt plus completion. Known missing/unsupported model
  responses produce partial/unknown coverage; missing usage is never zero.
- Native LangChain/provider telemetry and other model node types are not
  comprehensively supported. Only recognized outputs contribute; reports are
  not a complete provider invoice or universal model detector. Repeated output
  items are not proof of separate billable calls.
- Duplicate execution exports are deduplicated by execution id or content hash;
  conflicting duplicates fail. Explicit mismatched workflow ids fail.
- Raw parameters, prompts, response content, headers, and API keys are not
  report fields. Credential-shaped labels and the active API key are scrubbed,
  but arbitrary human labels/URLs can contain personal or sensitive data.
  **Review reports manually before sharing. Never share private configuration.**

Verification on 2026-11-05 included actual Replant record/replay, stale/artifact
checks, GET-only API fixtures, secret handling, concurrency, and public-fetch
boundaries. Scanning the saved real n8n benchmark export yielded 100 recognized
responses and 45,200 observed tokens, with customer savings still unmeasured.
A real Makeitfuture page scan produced a source hash and unknown buying intent.
Neither verification executed customer workflows.

```sh
REPLANT_BIN="$PWD/target/release/replant" \
  python3 -m unittest discover -s scripts -p test_scan_automation.py -v
```
