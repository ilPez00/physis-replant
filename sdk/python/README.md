# physis-replant Python SDK

Stdlib-only client for the HTTP bridge.

```python
from replant_client import ReplantClient

client = ReplantClient("http://172.17.0.1:8799", token)
receipt = client.run({"prompt": "increment once", "input": {"amount": 1}, "context": {"workflow": "v1"}})
if receipt["status"] == "miss":
    record = planner(receipt)          # your LLM
    client.record(record)              # executes and verifies
elif receipt["status"] == "review":
    client.approve(receipt_request, receipt["candidates"][0]["id"])
elif receipt["status"] == "stale":
    ...                                # inspect stale_record; request authorization
elif receipt["status"] == "succeeded":
    ...                                # already re-executed; no model call
```

Methods: `health`, `run`, `record`, `inspect`, `stats`, `approve`, `reject`,
`undo`, `rerecord`. Errors raise `ReplantError` with `.status` and `.receipt`,
including failed-tool stdout/stderr and exit codes returned by the bridge.

After reviewing tool changes and explicitly authorizing repeated effects:

```python
receipt = client.rerecord(record_id)  # POST /rerecord with {"id": record_id}
```

This executes real actions and checks; it is not a lookup or an automatic
fallback. A failed/interrupted or undone source must be inspected before a fresh
`record`, not silently retried. For long recipes, set the HTTP timeout above the
entire expected duration, for example `ReplantClient(url, token, timeout=1860)`
for a single 30-minute tool plus overhead. The registry independently limits each
tool to 30 minutes. A client timeout/disconnect leaves effects unknown; inspect
the journal and real state before doing anything again. No automatic retries.

`inspect(request)` calls read-only `POST /inspect`, returning the stored record
or `None`. Requires the updated bridge; never falls back to `/run`.

## Adapters

`sdk/python/adapters/` contains optional shims for LangChain (`langchain_core`),
CrewAI (`crewai`), and AutoGen (plain callable). They import lazily and raise an
`ImportError` with the install hint when the framework is absent. They are thin
conversion layers, not framework runtime-tested integrations.

## Test

```sh
python3 -m unittest -v test_client.py test_adapters.py
```
