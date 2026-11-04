"""Optional framework adapters for the Replant HTTP bridge.

Each adapter is a thin shim: the framework supplies the model/tool schema,
Replant supplies verified replay. Frameworks are optional dependencies; import
errors name what to install. These shims are not a substitute for a framework
runtime test.
"""
from __future__ import annotations

import json
from functools import partial
from typing import Any, Dict, Optional


def request_payload(
    prompt: str,
    input: Optional[Dict[str, Any]] = None,
    context: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    if not prompt or not prompt.strip():
        raise ValueError("prompt is required")
    return {"prompt": prompt, "input": input or {}, "context": context or {}}


def _run_json(client, prompt: str, input_json: str = "{}", context_json: str = "{}") -> str:
    request = request_payload(prompt, json.loads(input_json), json.loads(context_json))
    return json.dumps(client.run(request))


def langchain_tools(client):
    """Return one LangChain StructuredTool. Requires `langchain-core`."""
    try:
        from langchain_core.tools import StructuredTool
    except ImportError as error:
        raise ImportError(
            "LangChain adapter needs langchain-core: pip install langchain-core"
        ) from error
    run = partial(_run_json, client)
    return [
        StructuredTool.from_function(
            func=run,
            name="replant_run",
            description=(
                "Look up a verified Replant recipe for a prompt. Returns the receipt "
                "as JSON: status miss means call your planner, then use replant_record."
            ),
        )
    ]


def crewai_tool(client):
    """Return a CrewAI BaseTool. Requires `crewai`."""
    try:
        from crewai.tools import BaseTool
    except ImportError as error:
        raise ImportError(
            "CrewAI adapter needs crewai: pip install crewai"
        ) from error

    class ReplantRunTool(BaseTool):
        name: str = "replant_run"
        description: str = (
            "Look up a verified Replant recipe for a prompt. Returns the receipt as JSON."
        )

        def _run(self, prompt: str, input_json: str = "{}", context_json: str = "{}") -> str:
            return _run_json(client, prompt, input_json, context_json)

    return ReplantRunTool()


def autogen_tool(client):
    """Return a plain function AutoGen can register as a tool."""

    def replant_run(prompt: str, input_json: str = "{}", context_json: str = "{}") -> str:
        """Look up a verified Replant recipe. Returns the receipt as JSON."""
        return _run_json(client, prompt, input_json, context_json)

    return replant_run
