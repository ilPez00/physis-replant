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
}
```

Methods: `health`, `run`, `record`, `inspect`, `stats`, `approve`, `reject`,
`undo`. Errors throw `ReplantError` with `.status` and `.receipt`.

`inspect(request)` calls read-only `POST /inspect`, returning the stored record
or `null`. Requires the updated bridge; never falls back to `/run`.

Node 18+ (uses global `fetch`; override with `options.fetchImpl`).

## Test

```sh
node --test test.mjs
```
