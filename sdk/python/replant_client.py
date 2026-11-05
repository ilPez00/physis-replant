"""Stdlib client for the physis-replant HTTP bridge.

    from replant_client import ReplantClient
    client = ReplantClient("http://172.17.0.1:8799", token)
    receipt = client.run({"prompt": "...", "input": {}, "context": {}})
    if receipt["status"] == "miss":
        ...  # call your planner, then client.record(record)
"""
from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Any, Dict, Optional


class ReplantError(RuntimeError):
    """HTTP or protocol error. `status` and `receipt` are best-effort context."""

    def __init__(self, message: str, status: Optional[int] = None, receipt: Any = None):
        super().__init__(message)
        self.status = status
        self.receipt = receipt


class ReplantClient:
    def __init__(self, base_url: str, token: str, timeout: float = 30.0):
        if not base_url:
            raise ValueError("base_url is required")
        if not token:
            raise ValueError("token is required")
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.timeout = timeout

    def _request(self, method: str, path: str, payload: Any = None) -> Any:
        data = None if payload is None else json.dumps(payload).encode()
        request = urllib.request.Request(
            self.base_url + path,
            data=data,
            method=method,
            headers={
                "Authorization": "Bearer " + self.token,
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                return json.loads(response.read())
        except urllib.error.HTTPError as error:
            body = error.read()
            try:
                receipt = json.loads(body)
            except ValueError:
                receipt = body.decode(errors="replace")
            raise ReplantError(
                f"replant {method} {path} failed: HTTP {error.code}",
                status=error.code,
                receipt=receipt,
            ) from error
        except urllib.error.URLError as error:
            raise ReplantError(f"replant {method} {path} failed: {error.reason}") from error

    def health(self) -> Dict[str, Any]:
        return self._request("GET", "/healthz")

    def run(self, request: Dict[str, Any]) -> Dict[str, Any]:
        return self._request("POST", "/run", request)

    def record(self, record: Dict[str, Any]) -> Dict[str, Any]:
        return self._request("POST", "/record", record)

    def inspect(self, request: Dict[str, Any]) -> Any:
        return self._request("POST", "/inspect", request)

    def stats(self) -> Dict[str, Any]:
        return self._request("GET", "/stats")

    def approve(self, request: Dict[str, Any], candidate: str) -> Dict[str, Any]:
        return self._request("POST", "/approve", {"request": request, "candidate": candidate})

    def reject(self, request: Dict[str, Any], candidate: str) -> Dict[str, Any]:
        return self._request("POST", "/reject", {"request": request, "candidate": candidate})

    def undo(self, record_id: str) -> Dict[str, Any]:
        return self._request("POST", "/undo", {"id": record_id})
