#!/usr/bin/env python3
"""Read-only n8n/prospect audits. Receipts attest the scan, not customer savings."""
import argparse
import fcntl
import hashlib
import http.client
import ipaddress
import json
import os
import pathlib
import re
import secrets
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.parse
from html.parser import HTMLParser

MAX_BODY = 16 << 20
TIMEOUT = 10


def digest(value):
    data = value if isinstance(value, bytes) else json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(data).hexdigest()


def label(value):
    text = str(value)
    text = re.sub(r"(?:sk-|ghp_|gho_|xoxb-|AIza)[A-Za-z0-9_-]{12,}", "[redacted]", text)
    text = re.sub(r"(?i)(?:bearer\s+|(?:api[_-]?key|token|password|secret)\s*[=:]\s*)[^\s]+", "[redacted]", text)
    return re.sub(r"[\x00-\x1f`|]", " ", text)[:160]


def parsed_url(url, public=False):
    p = urllib.parse.urlsplit(url)
    if not p.hostname or p.username or p.password or p.fragment or any(ord(c) < 33 for c in url):
        raise ValueError("URL must have a host and no credentials, fragment or control characters")
    if p.scheme not in ("http", "https"):
        raise ValueError("only HTTP(S) URLs are supported")
    if public and (p.scheme != "https" or p.query or p.port not in (None, 443)):
        raise ValueError("public scans require HTTPS without query parameters or custom ports")
    return p


class PublicHTTPS(http.client.HTTPSConnection):
    def __init__(self, host, address):
        super().__init__(host, timeout=TIMEOUT, context=ssl.create_default_context())
        self.address = address

    def connect(self):
        # Pin validated DNS result while keeping certificate verification and SNI.
        raw = socket.create_connection((self.address, 443), self.timeout)
        try:
            self.sock = self._context.wrap_socket(raw, server_hostname=self.host)
        except Exception:
            raw.close()
            raise


def fetch(url, headers=None, public=False):
    p = parsed_url(url, public)
    if public:
        addresses = {r[4][0] for r in socket.getaddrinfo(p.hostname, 443, type=socket.SOCK_STREAM)}
        if not addresses or not all(ipaddress.ip_address(a).is_global for a in addresses):
            raise ValueError("public scan refused a private/reserved target")
        connection = PublicHTTPS(p.hostname, sorted(addresses)[0])
    else:
        if p.scheme == "http":
            # IP literals avoid a second DNS lookup exposing cleartext credentials.
            try:
                address = ipaddress.ip_address(p.hostname)
            except ValueError:
                raise ValueError("HTTP API requires a non-public IP literal; use HTTPS for hostnames") from None
            if address.is_global or address.is_unspecified or address.is_multicast:
                raise ValueError("HTTP API requires a non-public unicast IP literal")
        connection = (http.client.HTTPSConnection if p.scheme == "https" else http.client.HTTPConnection)(
            p.hostname, p.port, timeout=TIMEOUT)
    try:
        connection.request("GET", urllib.parse.urlunsplit(("", "", p.path or "/", p.query, "")),
                           headers={"User-Agent": "physis-replant-audit/1", **(headers or {})})
        response = connection.getresponse()
        if response.status != 200:
            raise ValueError(f"HTTP {response.status}; stopped without redirect or retry")
        body = response.read(MAX_BODY + 1)
        if len(body) > MAX_BODY:
            raise ValueError("source exceeds body size limit")
        if response.length not in (None, 0):
            raise ValueError("source ended before its declared Content-Length")
        return body, response.getheader("Content-Type", "")
    finally:
        connection.close()


def model_node(node):
    kind = node.get("type", "").lower()
    url = str(node.get("parameters", {}).get("url", "")).lower()
    return any(s in kind for s in ("lmchat", ".openai", ".anthropic", ".gemini")) or (
        kind.endswith(".httprequest") and any(s in url for s in ("/chat/completions", "/messages", ":generatecontent")))


def analyze_workflows(workflows, executions):
    summaries, maps = [], {}
    if not workflows or len(workflows) > 100:
        raise ValueError("provide 1..100 workflows")
    for wf in workflows:
        nodes = wf.get("nodes")
        if not isinstance(nodes, list) or len(nodes) > 512 or not isinstance(wf.get("connections", {}), dict):
            raise ValueError("invalid workflow shape or more than 512 nodes")
        by_name = {n["name"]: n for n in nodes}
        if len(by_name) != len(nodes):
            raise ValueError("duplicate node names")
        graph = {name: set() for name in by_name}
        for source, channels in wf.get("connections", {}).items():
            for channel, outputs in channels.items():
                for output in outputs:
                    for edge in output or []:
                        target = edge["node"]
                        if source in graph and target in graph:
                            graph[source].add(target)
                            if channel == "ai_languageModel":
                                graph[target].add(source)

        def reaches(start, target):
            pending, seen = list(graph[start]), set()
            while pending:
                item = pending.pop()
                if item == target:
                    return True
                if item not in seen:
                    seen.add(item)
                    pending.extend(graph[item] - seen)
            return False

        models = [n for n in nodes if model_node(n)]
        loops = [n for n in nodes if n.get("type", "").endswith(".splitInBatches")]
        candidates = [{"loop": label(loop["name"]), "model": label(model["name"]), "status": "candidate",
                       "reason": "model and loop share a cyclic region; equivalent inputs and safe replay are unverified"}
                      for loop in loops for model in models
                      if reaches(loop["name"], model["name"]) and reaches(model["name"], loop["name"])]
        summaries.append({"workflow": label(wf.get("name", wf.get("id", "unnamed"))),
                          "nodes": len(nodes), "model_nodes": len(models), "loop_candidates": candidates})
        maps[str(wf.get("id", ""))] = by_name

    totals = {k: 0 for k in ("prompt_tokens", "completion_tokens", "total_tokens")}
    measured = missing = duplicates = 0
    seen = {}
    responses, anonymous_responses = {}, set()
    for ex in executions:
        if (not isinstance(ex, dict) or not isinstance(ex.get("data"), dict)
                or not isinstance(ex["data"].get("resultData"), dict)
                or not isinstance(ex["data"]["resultData"].get("runData"), dict)):
            raise ValueError("unsupported execution export; require data.resultData.runData")
        signature = digest(ex)
        key = str(ex["id"]) if ex.get("id") is not None else signature
        if key in seen:
            if seen[key] != signature:
                raise ValueError("conflicting duplicate execution id")
            duplicates += 1
            continue
        seen[key] = signature
        node_map = maps.get(str(ex.get("workflowId", "")))
        if node_map is None:
            if ex.get("workflowId") is not None or len(workflows) != 1:
                raise ValueError("cannot attribute execution to a workflow")
            node_map = next(iter(maps.values()))
        for name, runs in ex["data"]["resultData"]["runData"].items():
            if not isinstance(runs, list):
                raise ValueError("execution node runs must be a list")
            node = node_map.get(name, {})
            is_model = model_node(node)
            is_http = node.get("type", "").endswith(".httpRequest")
            if not runs and (is_model or name not in node_map):
                missing += 1
            for run in runs:
                if not isinstance(run, dict) or not isinstance(run.get("data", {}), dict):
                    raise ValueError("execution run/data must be objects")
                outputs = run.get("data", {}).get("main", [])
                if not isinstance(outputs, list) or any(output is not None and not isinstance(output, list) for output in outputs):
                    raise ValueError("execution main outputs must be arrays")
                items = [item for output in outputs for item in output or []]
                if any(not isinstance(item, dict) or not isinstance(item.get("json", {}), dict) for item in items):
                    raise ValueError("execution output items/json must be objects")
                if name not in node_map:
                    missing += max(1, len(items))
                    continue
                if is_model and not items:
                    missing += 1
                for item in items:
                    value = item.get("json", {})
                    if isinstance(value, dict) and isinstance(value.get("body"), dict):
                        value = value["body"]
                    response = isinstance(value, dict) and isinstance(value.get("choices"), list)
                    if not is_model and not (is_http and response):
                        continue
                    usage = value.get("usage", {}) if isinstance(value, dict) else {}
                    valid = is_http and response and isinstance(usage, dict) and all(type(usage.get(k)) is int and usage[k] >= 0 for k in totals)
                    if valid and usage["total_tokens"] == usage["prompt_tokens"] + usage["completion_tokens"]:
                        provider = str(node.get("parameters", {}).get("url", ""))
                        response_id, signature = value.get("id"), digest(value)
                        if isinstance(response_id, str) and response_id:
                            identity = (provider, response_id)
                            if identity in responses:
                                if responses[identity] != signature:
                                    raise ValueError("conflicting provider response identity")
                                continue
                            responses[identity] = signature
                        else:
                            identity = (provider, signature)
                            if identity in anonymous_responses:
                                missing += 1  # Cannot distinguish copied output from another billable call.
                                continue
                            anonymous_responses.add(identity)
                        measured += 1
                        for field in totals:
                            totals[field] += usage[field]
                    else:
                        missing += 1
    return {"workflows": summaries,
            "usage": {"status": "partial" if missing else "observed" if measured else "unknown",
                      "observed_model_responses": measured, "unmeasured_responses": missing,
                      "known_tokens": totals, "total_tokens": totals["total_tokens"] if measured and not missing else None,
                      "duplicate_exports_skipped": duplicates},
            "customer_savings": None, "replay_eligibility": "unverified", "customer_workflows_executed": False}


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.hidden = 0
        self.in_title = False
        self.title, self.text = [], []

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style"):
            self.hidden += 1
        if tag == "title":
            self.in_title = True

    def handle_endtag(self, tag):
        if tag in ("script", "style"):
            self.hidden = max(0, self.hidden - 1)
        if tag == "title":
            self.in_title = False

    def handle_data(self, text):
        if not self.hidden:
            self.text.append(text)
            if self.in_title:
                self.title.append(text)


def read_json(body):
    try:
        return json.loads(body)
    except (ValueError, UnicodeError):
        raise ValueError("source did not contain valid JSON") from None


def collect(config):
    mode, sources = config["mode"], []
    if mode == "prospects":
        urls = config["urls"]
        if not 1 <= len(urls) <= 5:
            raise ValueError("provide 1..5 explicit public URLs per scan")
        prospects = []
        for url in urls:
            body, content_type = fetch(url, public=True)
            if not any(kind in content_type.lower() for kind in ("text/html", "text/plain")):
                raise ValueError("prospect URL must return HTML or plain text")
            page = Page()
            page.feed(body.decode("utf-8", errors="replace"))
            text = " ".join(page.text).lower()
            prospects.append({"url": url, "page_title": label(" ".join(page.title)),
                              "matched_terms": [word for word in ("n8n", "automation", "ai agent", "token", "workflow") if word in text],
                              "source_sha256": digest(body), "buying_intent": "unknown", "contacted": False})
        return {"prospects": prospects, "customer_savings": None, "replay_eligibility": "unverified", "customer_workflows_executed": False}

    if mode == "api":
        base = config["instance"].rstrip("/")
        p = parsed_url(base)
        if p.query or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", config["workflow_id"]):
            raise ValueError("invalid instance URL or workflow id")
        if not re.fullmatch(r"[A-Z_][A-Z0-9_]*", config["key_env"]):
            raise ValueError("API key must be named by an environment variable")
        key = os.environ.get(config["key_env"])
        if not key or not 16 <= len(key) <= 8192:
            raise ValueError("API key environment variable must contain 16..8192 characters")
        limit = config.get("limit", 100)
        if type(limit) is not int or not 1 <= limit <= 100:
            raise ValueError("execution limit must be 1..100")
        query = urllib.parse.urlencode({"workflowId": config["workflow_id"], "includeData": "true", "limit": limit})
        bodies = [fetch(base + suffix, {"X-N8N-API-KEY": key})[0] for suffix in (
            "/api/v1/workflows/" + config["workflow_id"], "/api/v1/executions?" + query)]
        wf, page = map(read_json, bodies)
        if not isinstance(wf, dict) or str(wf.get("id")) != config["workflow_id"]:
            raise ValueError("workflow API returned a different workflow")
        if not isinstance(page, dict) or not isinstance(page.get("data"), list):
            raise ValueError("unsupported execution API page")
        wf = json.loads(json.dumps(wf).replace(json.dumps(key)[1:-1], "[redacted]"))
        workflows, executions = [wf], page["data"]
        complete = not bool(page.get("nextCursor"))
        sources = [{"kind": kind, "sha256": digest(body)} for kind, body in zip(("workflow_api", "executions_api"), bodies)]
    elif mode == "files":
        parsed = []
        for source in config["files"]:
            with pathlib.Path(source["path"]).open("rb") as stream:
                body = stream.read(MAX_BODY + 1)
            if len(body) > MAX_BODY or digest(body) != source["sha256"]:
                raise ValueError("source size or hash changed; start a new scan")
            try:
                parsed.append(json.loads(body))
            except (ValueError, UnicodeError):
                from n8n_runtime_benchmark import execution_result
                parsed.append(execution_result(body.decode("utf-8")))
            sources.append({"kind": source["kind"], "sha256": source["sha256"]})
        first = parsed[0]
        workflows = first if isinstance(first, list) else first.get("data", [first])
        executions = []
        if len(parsed) > 1:
            second = parsed[1]
            executions = second if isinstance(second, list) else second.get("data", [second]) if isinstance(second.get("data"), list) else [second]
        complete = True
    else:
        raise ValueError("unknown scan mode")
    report = analyze_workflows(workflows, executions)
    report.update(sources=sources, coverage={"complete": complete, "scope": "supplied exports or selected workflow execution page; not account-wide"})
    if mode == "api":
        # Known active credential must not escape even if an upstream label echoes it.
        report = json.loads(json.dumps(report).replace(json.dumps(key)[1:-1], "[redacted]"))
    return report


OWNER = "replant-read-only-audit-v1"


def write_atomic(path, body):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = pathlib.Path(stream.name)
        try:
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            if temporary.exists():
                temporary.unlink()  # Only this invocation's own temporary file.


def write_json(path, value):
    write_atomic(path, (json.dumps(value, sort_keys=True, indent=2) + "\n").encode())


def private_json(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "rb") as stream:
        body = stream.read(MAX_BODY + 1)
    if len(body) > MAX_BODY:
        raise ValueError("private artifact exceeds size limit")
    return read_json(body)


def owned_root(path, initialize=False):
    if path.is_symlink():
        raise ValueError("output root must not be a symlink")
    if initialize:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    metadata = path.stat()
    if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError("output root must be owned by you and private (0700)")
    marker = path / "owner.json"
    if initialize and not marker.exists():
        if any(path.iterdir()):
            raise ValueError("refusing unrelated existing output files")
        write_json(marker, {"format": OWNER})
    if private_json(marker) != {"format": OWNER}:
        raise ValueError("output directory does not belong to this scanner")
    for name in ("config.json", "attempt.json", "report.json", "report.md", "report.sha256", "latest.json", "receipts", "state", "tools.json"):
        if (path / name).is_symlink():
            raise ValueError("refusing symlink in owned output directory")
    return path


def markdown(report):
    return ("# Read-only automation audit\n\n"
            "This receipt verifies the scan, not execution of customer workflows.\n"
            "Static opportunities are candidates. Customer savings and safe replay remain unmeasured.\n"
            "Review labels before sharing; no parameters, prompts or authorization headers are included.\n\n"
            "```json\n" + json.dumps(report, sort_keys=True, indent=2) + "\n```\n")


def worker(phase, root, payload):
    owned_root(root)
    config = private_json(root / "config.json")
    expected = digest(config)
    attempt = private_json(root / "attempt.json")
    if payload != {"config_sha256": expected} or attempt["config_sha256"] != expected:
        raise ValueError("configuration/challenge binding mismatch")
    if phase == "_scan":
        findings = collect(config)
        report = {"schema": 1, "receipt_scope": "read_only_scan_not_customer_execution",
                  "config_sha256": expected, "challenge": attempt["nonce"], "created_at_ns": time.time_ns(),
                  "findings": findings}
        write_json(root / "report.json", report)
        write_atomic(root / "report.md", markdown(report).encode())
        artifact = digest((root / "report.json").read_bytes())
        write_atomic(root / "report.sha256", artifact.encode())
    elif phase == "_check":
        report = private_json(root / "report.json")
        if report.get("schema") != 1 or report.get("receipt_scope") != "read_only_scan_not_customer_execution":
            raise ValueError("invalid audit artifact schema/scope")
        if report.get("challenge") != attempt["nonce"] or report.get("config_sha256") != expected:
            raise ValueError("artifact belongs to a different scan attempt")
        findings = report["findings"]
        if findings.get("customer_savings") is not None or findings.get("customer_workflows_executed") is not False or findings.get("replay_eligibility") != "unverified":
            raise ValueError("audit artifact asserts an unmeasured customer outcome")
        if config["mode"] != "prospects":
            usage = findings["usage"]
            counts = [usage[k] for k in ("observed_model_responses", "unmeasured_responses", "duplicate_exports_skipped")]
            tokens = usage["known_tokens"]
            values = [tokens[k] for k in ("prompt_tokens", "completion_tokens", "total_tokens")]
            if any(type(v) is not int or v < 0 for v in counts + values) or values[2] != values[0] + values[1]:
                raise ValueError("inconsistent usage counts or token totals")
            measured, missing, _ = counts
            status = "partial" if missing else "observed" if measured else "unknown"
            total = values[2] if measured and not missing else None
            if usage["status"] != status or usage["total_tokens"] != total or (not measured and any(values)):
                raise ValueError("usage status/totals disagree with measured coverage")
        for source in findings.get("sources", []) + findings.get("prospects", []):
            value = source.get("sha256", source.get("source_sha256", ""))
            if not re.fullmatch(r"[a-f0-9]{64}", value):
                raise ValueError("missing source evidence hash")
        if config["mode"] == "files":
            expected_sources = [{"kind": s["kind"], "sha256": s["sha256"]} for s in config["files"]]
            if findings.get("sources") != expected_sources:
                raise ValueError("artifact source binding does not match configured inputs")
            for source in config["files"]:
                with pathlib.Path(source["path"]).open("rb") as stream:
                    body = stream.read(MAX_BODY + 1)
                if len(body) > MAX_BODY or digest(body) != source["sha256"]:
                    raise ValueError("source changed during scan")
        if (root / "report.md").read_text() != markdown(report):
            raise ValueError("human-readable report does not match JSON artifact")
        artifact = digest((root / "report.json").read_bytes())
        if (root / "report.sha256").read_text() != artifact:
            raise ValueError("audit artifact hash mismatch")
    else:
        raise ValueError("unknown worker phase")
    return {"ok": True, "artifact_sha256": artifact}


def audit(config, out, binary, force_record=False):
    root = owned_root(out, initialize=True).resolve()
    fd = os.open(root / "scan.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        owned_root(root)
        config_hash = digest(config)
        write_json(root / "config.json", config)
        nonce = secrets.token_hex(32)
        write_json(root / "attempt.json", {"nonce": nonce, "config_sha256": config_hash})
        write_json(root / "latest.json", {"status": "started", "challenge": nonce})
        tools = {name: {"kind": kind, "program": str(pathlib.Path(sys.executable).resolve()),
                       "args": [str(pathlib.Path(__file__).resolve()), phase, str(root)],
                       "fingerprint_files": [str(pathlib.Path(__file__).resolve()), str(root / "config.json")],
                       "timeout_ms": 120000}
                 for name, kind, phase in (("audit_scan", "action", "_scan"), ("audit_check", "check", "_check"))}
        write_json(root / "tools.json", tools)
        request = {"prompt": "Read-only automation audit", "input": {"config_sha256": config_hash},
                   "context": {"audit": OWNER, "config_sha256": config_hash}}
        record = {"request": request, "recipe": {"steps": [{"tool": "audit_scan", "input": request["input"]}],
                  "checks": [{"tool": "audit_check", "input": request["input"]}]},
                  "trace": {"provider": "none:deterministic-read-only-audit", "config_sha256": config_hash}}

        def invoke(verb, value):
            result = subprocess.run([str(binary), verb, "--store", str(root / "state"), "--tools", str(root / "tools.json")],
                input=json.dumps(value), capture_output=True, text=True, timeout=260)
            receipt = json.loads(result.stdout)
            if not receipt.get("id"):
                raise ValueError("Replant rejected the request before producing a receipt")
            return receipt

        receipt = invoke("record", record) if force_record else invoke("run", request)
        if receipt["status"] == "miss":
            receipt = invoke("record", record)
        artifact = None
        if receipt["status"] == "succeeded":
            checked = worker("_check", root, request["input"])
            artifact = checked["artifact_sha256"]
            if any(call.get("output", {}).get("artifact_sha256") != artifact for call in receipt["calls"]):
                raise ValueError("receipt outputs do not match current artifact")
        if not re.fullmatch(r"[a-f0-9]{64}", receipt["id"]):
            raise ValueError("invalid receipt id")
        receipts = root / "receipts"
        receipts.mkdir(mode=0o700, exist_ok=True)
        bundle = {"receipt": receipt, "artifact_sha256": artifact,
                  "receipt_scope": "scanner_execution_only", "customer_savings": None}
        write_json(receipts / (receipt["id"] + ".json"), bundle)
        write_json(root / "latest.json", bundle)
        return bundle


def main():
    os.umask(0o077)
    if len(sys.argv) > 1 and sys.argv[1] in ("_scan", "_check"):
        try:
            body = sys.stdin.buffer.read(65537)
            if len(body) > 65536:
                raise ValueError("worker input exceeds 64 KiB")
            result = worker(sys.argv[1], pathlib.Path(sys.argv[2]), read_json(body))
            code = 0
        except Exception as error:
            result, code = {"ok": False, "error": label(error)}, 1
        print(json.dumps(result))
        return code
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("files", "api", "prospects"))
    parser.add_argument("--out", required=True, type=pathlib.Path)
    parser.add_argument("--replant", default=os.environ.get("REPLANT_BIN") or shutil.which("replant") or str(pathlib.Path(__file__).resolve().parents[1] / "target/release/replant"))
    parser.add_argument("--workflow", type=pathlib.Path)
    parser.add_argument("--executions", type=pathlib.Path)
    parser.add_argument("--instance-url")
    parser.add_argument("--workflow-id")
    parser.add_argument("--key-env", default="N8N_API_KEY")
    parser.add_argument("--limit", type=int, default=100)
    parser.add_argument("--url", action="append")
    parser.add_argument("--record", action="store_true", help="Explicit re-record after inspecting a failed/blocked scan")
    args = parser.parse_args()
    try:
        config = {"mode": args.mode}
        if args.mode == "files":
            if not args.workflow:
                raise ValueError("files mode needs --workflow")
            sources = []
            for path, kind in ((args.workflow, "workflow"), (args.executions, "executions")):
                if path:
                    with path.open("rb") as stream:
                        body = stream.read(MAX_BODY + 1)
                    if len(body) > MAX_BODY:
                        raise ValueError("input file exceeds size limit")
                    sources.append({"path": str(path.resolve()), "kind": kind, "sha256": digest(body)})
            config["files"] = sources
        elif args.mode == "api":
            if not args.instance_url or not args.workflow_id:
                raise ValueError("api mode needs --instance-url and --workflow-id")
            p = parsed_url(args.instance_url)
            if p.query or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", args.workflow_id) or not re.fullmatch(r"[A-Z_][A-Z0-9_]*", args.key_env) or not 1 <= args.limit <= 100:
                raise ValueError("invalid API instance, workflow id, key variable or limit")
            config.update(instance=args.instance_url, workflow_id=args.workflow_id, key_env=args.key_env, limit=args.limit)
        else:
            if not args.url:
                raise ValueError("prospects mode needs explicit --url arguments")
            if not 1 <= len(args.url) <= 5:
                raise ValueError("provide 1..5 URLs")
            for url in args.url:
                parsed_url(url, public=True)
            config["urls"] = args.url
        bundle = audit(config, args.out, pathlib.Path(args.replant).resolve(), args.record)
        print(json.dumps(bundle))
        return 0 if bundle["receipt"]["status"] == "succeeded" else 1
    except Exception as error:
        print(json.dumps({"status": "failed", "error": label(error)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
