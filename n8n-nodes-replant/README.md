# n8n-nodes-replant

Community node for the Replant HTTP bridge.

## Install

For a self-hosted n8n with community nodes enabled:

```sh
mkdir -p ~/.n8n/custom && cd ~/.n8n/custom
npm install /path/to/n8n-nodes-replant
```

Restart n8n. The **Replant** node appears in the node panel. This package has no
runtime dependencies beyond Node's `http` helper provided by n8n.

## Credentials

Create a **Replant API** credential:

- **Base URL** — bridge address, e.g. `http://172.17.0.1:8799`
- **Bearer Token** — the bridge token

The credential test calls `GET /healthz`.

## Operations

| Operation | Endpoint | Payload |
|---|---|---|
| Run | `POST /run` | `{prompt,input,context}` |
| Record | `POST /record` | `{request,recipe,trace}` |
| Re-record (Repeat Effects) | `POST /rerecord` | `{id}` |
| Inspect | `POST /inspect` | `{prompt,input,context}` |
| Stats | `GET /stats` | — |
| Approve Near-Match | `POST /approve` | `{request,candidate}` |
| Reject Near-Match | `POST /reject` | `{request,candidate}` |
| Undo | `POST /undo` | `{id}` |

Route `status: miss` to your planner and then Record. Route `review` to a human
or policy step; never auto-approve. Route `stale` to an operator to inspect the
stored template/id and authorize any repeated effects. Re-record is explicit;
never automatically connect `stale` to it. `blocked`/`failed` stop the workflow.

Set **Request Timeout (Milliseconds)** above the entire authorized recipe's
expected duration (default 30,000; accepted range 1..86,400,000). For a single
30-minute tool plus overhead, 1,860,000 is an example; multiple long tools may
require more. The operator-owned registry separately limits each tool to
30 minutes. Timeout/disconnect does not prove that effects were cancelled.
Inspect the journal and real state before retrying; retries are not automatic.

Each input item produces one linked output item; payload expressions are
evaluated per item. Inspect never executes a recipe and returns
`{record: <stored record or null>}`. Inspection requires the updated HTTP bridge;
older bridges return 404, not a fallback to execution.

## Test

```sh
node --test test/node.test.js
```

The tests mock `this.helpers.httpRequest`; they do not run n8n itself.

## Limits

No automatic retries, rollback, or exactly-once behavior. Items execute
sequentially; if a later item fails, earlier effects remain. The bridge must be
reachable from the n8n container; see `examples/n8n/README.md` for the
container-to-host networking caveat.
