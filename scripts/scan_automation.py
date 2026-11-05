#!/usr/bin/env python3
"""Read-only n8n/prospect audits. Receipts attest the scan, not customer savings."""
import argparse
import hashlib
import http.client
import ipaddress
import json
import os
import pathlib
import re
import socket
import ssl
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
            # Cleartext API credentials only to an explicitly selected local/private endpoint.
            addresses = {r[4][0] for r in socket.getaddrinfo(p.hostname, p.port or 80, type=socket.SOCK_STREAM)}
            if not addresses or any(ipaddress.ip_address(a).is_global for a in addresses):
                raise ValueError("public API instances require HTTPS")
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
    for ex in executions:
        if not isinstance(ex, dict) or not isinstance(ex.get("data", {}).get("resultData", {}).get("runData"), dict):
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
            if len(workflows) != 1:
                raise ValueError("cannot attribute execution to a workflow")
            node_map = next(iter(maps.values()))
        for name, runs in ex["data"]["resultData"]["runData"].items():
            node = node_map.get(name, {})
            is_model = model_node(node)
            is_http = node.get("type", "").endswith(".httpRequest")
            for run in runs:
                items = [item for output in run.get("data", {}).get("main", []) for item in output or []]
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
                    valid = is_http and response and all(type(usage.get(k)) is int and usage[k] >= 0 for k in totals)
                    if valid and usage["total_tokens"] == usage["prompt_tokens"] + usage["completion_tokens"]:
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
            body, _ = fetch(url, public=True)
            page = Page()
            page.feed(body.decode("utf-8", errors="replace"))
            text = " ".join(page.text).lower()
            prospects.append({"url": url, "page_title": label(" ".join(page.title)),
                              "matched_terms": [word for word in ("n8n", "automation", "ai agent", "token", "workflow") if word in text],
                              "source_sha256": digest(body), "buying_intent": "unknown", "contacted": False})
        return {"prospects": prospects, "customer_savings": None, "customer_workflows_executed": False}

    if mode == "api":
        base = config["instance"].rstrip("/")
        p = parsed_url(base)
        if p.query or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", config["workflow_id"]):
            raise ValueError("invalid instance URL or workflow id")
        if not re.fullmatch(r"[A-Z_][A-Z0-9_]*", config["key_env"]):
            raise ValueError("API key must be named by an environment variable")
        key = os.environ.get(config["key_env"])
        if not key:
            raise ValueError("API key environment variable is missing")
        limit = config.get("limit", 100)
        if type(limit) is not int or not 1 <= limit <= 100:
            raise ValueError("execution limit must be 1..100")
        query = urllib.parse.urlencode({"workflowId": config["workflow_id"], "includeData": "true", "limit": limit})
        bodies = [fetch(base + suffix, {"X-N8N-API-KEY": key})[0] for suffix in (
            "/api/v1/workflows/" + config["workflow_id"], "/api/v1/executions?" + query)]
        wf, page = map(read_json, bodies)
        if not isinstance(page, dict) or not isinstance(page.get("data"), list):
            raise ValueError("unsupported execution API page")
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
    return report
