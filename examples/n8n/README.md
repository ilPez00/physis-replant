# n8n integration (self-hosted, local)

> Prefer the community node in [`n8n-nodes-replant/`](../../n8n-nodes-replant/).
> The HTTP workflow below remains a zero-install fallback and was the path used
> in the live record→replay test.

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

### Actual n8n loop with a live model

[`scripts/n8n_runtime_benchmark.py`](../../scripts/n8n_runtime_benchmark.py)
imports and executes two real workflows using the installed n8n CLI. Unlike the
original webhook benchmark, the model request is an **HTTP Request node inside
n8n**, not an external Python planner.

Both workflows use **Loop Over Items**, batch size 1:

```text
Baseline: item -> LLM Plan -> Record (action + check) -> next item
Replant:  item -> Lookup -> succeeded ----------------> next item
                         -> miss -> LLM Plan -> Record -> next item
```

Any other lookup status stops the test. The benchmark rejects missing token
telemetry, incomplete loops, unexpected operation sequences, extra/missing
actions, and missing/failed checks. HTTP node retries are disabled.

With a working n8n CLI, an unauthenticated local OpenAI-compatible model endpoint,
and a built release binary:

```sh
REPLANT_LLM_URL=http://YOUR_GATEWAY/v1/chat/completions \
REPLANT_LLM_MODEL=YOUR_MODEL \
python3 scripts/n8n_runtime_benchmark.py \
  --host "$(tailscale ip -4)" \
  --iterations 100 \
  --directory "/tmp/opencode/replant-n8n-$(date +%s)"
```

`--host` names one private IPv4 interface reachable by n8n. The directory must
not exist. Workflows, execution logs, isolated databases and `report.json` stay
inside it. The temporary bridge is stopped afterward. No deployed workflow,
store, counter or credential is reused. n8n's environment access is enabled only
in this isolated process so HTTP nodes can read the temporary bridge token.

The script requires a complete n8n installation, including its native SQLite
binding. It adds no n8n/SQLite dependency to Replant. This is a real **CLI-mode
n8n workflow** test; it does not test the Docker/webhook deployment or the
community-node package.

## 6. Secret handling

The template uses a placeholder. When you fill it, the bearer token lands in the
n8n workflow and database. For a shared instance, use an n8n `httpHeaderAuth`
credential instead and keep the token out of the export. Do not commit a filled
workflow to a public repo.

## 7. Limits

- The original webhook template has a literal placeholder planner. The CLI
  benchmark above performs live inference inside n8n.
- No rollback or exactly-once guarantee. A failed check can follow a completed
  action; inspect effects before re-recording.
- One store = one bridge; the bridge serializes state with an OS lock.
- The Cloudflare tunnel is temporary and public; use a host-network container
  or a firewall rule for production.
