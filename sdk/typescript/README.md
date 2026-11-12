# @physis-replant/sdk

TypeScript-ready client: plain ESM JavaScript plus `replant.d.ts`. No build step.

```ts
import { ReplantClient, ReplantError } from '@physis-replant/sdk';

const client = new ReplantClient('http://172.17.0.1:8799', token);
const receipt = await client.run({
  prompt: 'increment once',
  input: { amount: 1 },
  context: { workflow: 'v1' },
});
if (receipt.status === 'miss') {
  await client.record(await planner(receipt)); // your LLM
} else if (receipt.status === 'review') {
  await client.approve(request, receipt.candidates![0].id);
} else if (receipt.status === 'stale') {
  // Inspect receipt.stale_record; do not automatically re-record.
}
```

Methods: `health`, `run`, `record`, `inspect`, `stats`, `approve`, `reject`,
`undo`, `rerecord`. Errors throw `ReplantError` with `.status` and `.receipt`,
including failed-tool diagnostic evidence. Types include `StaleRecord`,
`ToolOutcome`, role-aware `ToolSpec`, and the opt-in `Recipe.bind_inputs` field.

`await client.rerecord(recordId)` explicitly repeats the stored request's real
actions and checks under current tools. Use only after reviewing the drift and
authorizing effects; never automatically after `stale`, failure or timeout.
Failed/interrupted/undone histories require inspection and a fresh `record`.

For a long recipe, configure the client timeout above its entire duration:

```ts
const client = new ReplantClient(url, token, { timeoutMs: 1_860_000 });
```

That example allows a single 30-minute tool plus overhead; multi-tool recipes
may need longer. The registry still caps each tool at 30 minutes. An aborted
HTTP request does not establish cancellation of server-side effects. Inspect
the journal and real state; no automatic retries are performed.

`inspect(request)` calls read-only `POST /inspect`, returning the stored record
or `null`. Requires the updated bridge; never falls back to `/run`.

Node 18+ (uses global `fetch`; override with `options.fetchImpl`).

## Test

```sh
node --test test.mjs
```
