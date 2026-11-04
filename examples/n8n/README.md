# n8n integration (self-hosted, local)

Architecture: n8n runs in a **Docker container**; `replant` runs on the **host**.
The container's HTTP client cannot reach any host interface in this deployment
(`172.17.0.1`, the LAN IP and the Tailscale IP all hang), so the tested path uses
a short-lived Cloudflare tunnel to the host bridge:

```
n8n container ──HTTPS──▶ <random>.trycloudflare.com ──▶ 172.17.0.1:8799 (replant serve)
```

This tunnel is a **demo workaround**, not a production design. It exposes the
bridge publicly (bearer token required) and the URL changes when cloudflared
restarts. The durable fixes are: run `replant serve` in a container on n8n's
network, or allow container→host traffic in the host firewall.

## 1. Host bridge

The bridge is installed as a user service:

```sh
replant serve \
  --bind 172.17.0.1:8799 \
  --store /home/gio/.replant-n8n/state \
  --tools /home/gio/.replant-n8n/tools.json \
  --token-file /home/gio/.replant-n8n/token
```

- `--bind` must name one interface. `0.0.0.0`/`::` is refused.
- Token file mode `0600`, at least 16 characters.
- Store must be private (`0700`); it holds requests, traces and receipts.
- Routes: `GET /healthz` (no auth); `POST /run`, `POST /record` (bearer token).
- Status: `200` succeeded/miss, `409` blocked, `422` failed, `401` bad token.
- No TLS. Keep the bind narrow; the tunnel adds HTTPS.

Persistent user unit: `~/.config/systemd/user/replant-bridge.service`
(`systemctl --user status replant-bridge`).

## 2. Tunnel (only while the container cannot reach the host)

```sh
cloudflared tunnel --url http://172.17.0.1:8799 --no-autoupdate
# copy the https://<random>.trycloudflare.com URL from the output
```

The tunnel is unauthenticated at the edge; the bridge still requires the bearer
token for `/run` and `/record`. Stop the tunnel when done.

## 3. Tool registry

The model picks a registered name and supplies JSON on stdin; it never chooses an
executable or argv. Example (`/home/gio/.replant-n8n/tools.json`):

```json
{
  "increment": {
    "kind": "action",
    "program": "/usr/bin/python3",
    "args": ["/home/gio/.replant-n8n/tools.py", "increment", "/home/gio/.replant-n8n/counter"],
    "fingerprint_files": ["/home/gio/.replant-n8n/tools.py"],
    "timeout_ms": 2000
  },
  "verify": {
    "kind": "check",
    "program": "/usr/bin/python3",
    "args": ["/home/gio/.replant-n8n/tools.py", "verify", "/home/gio/.replant-n8n/counter"],
    "fingerprint_files": ["/home/gio/.replant-n8n/tools.py"],
    "timeout_ms": 2000
  }
}
```

Registry authorization is **not** a sandbox. Tools run as the bridge user and
must validate input and enforce their own tenant/resource permissions.

## 4. Workflow

Import [`replant-verified-counter.json`](replant-verified-counter.json), then
replace `<BRIDGE_URL>` and `<REPLANT_TOKEN>` in both HTTP Request nodes.

Flow:

```
Webhook → POST /run
            ├─ succeeded → Done
            └─ miss      → POST /record
```

- The template uses **literal** demo request/recipe bodies so it imports and
  runs without node-expression debugging. For dynamic requests, add a Set node
  and use expressions; verify them in this n8n version first.
- The "planner" is a literal placeholder, **not an LLM**. Replace the record
  body with your AI planner's typed `{request, recipe, trace}` output. Do not
  execute actions in the AI branch; `record` executes them.
- Replay executes the action and the independent check again. It does not
  return a cached side-effect response.
- `409` (blocked) and `422` (failed) stop the HTTP Request node by default.
  Inspect effects before recording again.

## 5. Tested evidence

On the running n8n 2.46.5 container (host `pansa`, 2026-11-04):

| webhook call | result | operation | counter |
| --- | --- | --- | --- |
| first | `succeeded` | `record` | 3 → 4 |
| second | `succeeded` | `replay`, `provider_calls:0` | 4 → 5 |

Both calls changed the real counter and the check passed. This proves the
n8n-compatible loop; it does **not** measure a live provider or token saving.

## 6. Secret handling

The template uses a placeholder. When you fill it, the bearer token lands in the
n8n workflow and database. For a shared instance, use an n8n `httpHeaderAuth`
credential instead and keep the token out of the export. Do not commit a filled
workflow to a public repo.

## 7. Limits

- No live provider call by this repo; the planner is external.
- No rollback or exactly-once guarantee. A failed check can follow a completed
  action; inspect effects before re-recording.
- One store = one bridge; the bridge serializes state with an OS lock.
- The Cloudflare tunnel is temporary and public; use a host-network container
  or a firewall rule for production.
