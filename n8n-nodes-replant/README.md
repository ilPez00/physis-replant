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
| Inspect | `POST /run` | `{prompt,input,context}` |
| Stats | `GET /stats` | — |
| Approve Near-Match | `POST /approve` | `{request,candidate}` |
| Reject Near-Match | `POST /reject` | `{request,candidate}` |
| Undo | `POST /undo` | `{id}` |

Route `status: miss` to your planner and then Record. Route `review` to a human
or policy step; never auto-approve. `blocked`/`failed` should stop the workflow.

## Test

```sh
node --test test/node.test.js
```

The tests mock `this.helpers.httpRequest`; they do not run n8n itself.

## Limits

No retries, batching, rollback, or exactly-once behavior. The bridge must be
reachable from the n8n container; see `examples/n8n/README.md` for the
container-to-host networking caveat.
