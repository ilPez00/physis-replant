//! Minimal MCP stdio server: newline-delimited JSON-RPC 2.0 over stdin/stdout.
//! Tools are thin projections over `Memo`; no domain logic lives here.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::workflow::{Memo, RecordInput, Request, ReviewInput, ToolRegistry};

pub const PROTOCOL_VERSION: &str = "2024-11-05";

fn schema(properties: Value, required: Vec<&str>) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn request_schema() -> Value {
    schema(
        json!({
            "prompt": {"type":"string"},
            "input": {"type":"object"},
            "context": {"type":"object"}
        }),
        vec!["prompt"],
    )
}

fn tool_list() -> Value {
    let review = schema(
        json!({
            "request": request_schema(),
            "candidate": {"type":"string","description":"64-hex candidate record id"}
        }),
        vec!["request", "candidate"],
    );
    json!({"tools":[
        {"name":"replant_run","description":"Look up an exact verified recipe; re-executes actions and checks on a hit. Returns miss or review candidates on a miss.","inputSchema":schema(json!({"request":request_schema()}),vec!["request"])},
        {"name":"replant_record","description":"Execute a planner-supplied recipe with independent checks and persist it if verified.","inputSchema":schema(json!({"record":schema(json!({"request":request_schema(),"recipe":{"type":"object"},"trace":{"type":"object"}}),vec!["request","recipe","trace"])}),vec!["record"])},
        {"name":"replant_inspect","description":"Return the learned record for an exact request, or null.","inputSchema":schema(json!({"request":request_schema()}),vec!["request"])},
        {"name":"replant_stats","description":"Return measured record/replay/review/undo counts.","inputSchema":schema(json!({}),vec![])},
        {"name":"replant_approve","description":"Execute a reviewed near-match candidate under this exact request and record it if checks pass.","inputSchema":review.clone()},
        {"name":"replant_reject","description":"Reject a near-match candidate for this request so it is not proposed again.","inputSchema":review},
        {"name":"replant_undo","description":"Run a learned record's declared undo steps in reverse order.","inputSchema":schema(json!({"id":{"type":"string","description":"64-hex record id"}}),vec!["id"])}
    ]})
}

fn text_result(value: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "{\"status\":\"failed\"}".into());
    json!({"content":[{"type":"text","text":text}],"isError":is_error})
}

fn call(memo: &mut Memo, name: &str, arguments: &Value) -> Result<Value, String> {
    let mut run = || -> Result<Value> {
        match name {
            "replant_run" => {
                let request: Request = serde_json::from_value(
                    arguments.get("request").cloned().unwrap_or(Value::Null),
                )
                .context("invalid request")?;
                Ok(serde_json::to_value(memo.run(&request)?)?)
            }
            "replant_record" => {
                let input: RecordInput =
                    serde_json::from_value(arguments.get("record").cloned().unwrap_or(Value::Null))
                        .context("invalid record")?;
                Ok(serde_json::to_value(memo.record(&input)?)?)
            }
            "replant_inspect" => {
                let request: Request = serde_json::from_value(
                    arguments.get("request").cloned().unwrap_or(Value::Null),
                )
                .context("invalid request")?;
                Ok(serde_json::to_value(memo.inspect(&request)?)?)
            }
            "replant_stats" => Ok(serde_json::to_value(memo.stats()?)?),
            "replant_approve" | "replant_reject" => {
                let input: ReviewInput =
                    serde_json::from_value(arguments.clone()).context("invalid review input")?;
                if name == "replant_approve" {
                    Ok(serde_json::to_value(memo.approve(&input)?)?)
                } else {
                    Ok(serde_json::to_value(memo.reject(&input)?)?)
                }
            }
            "replant_undo" => {
                let id = arguments
                    .get("id")
                    .and_then(Value::as_str)
                    .context("missing id")?;
                Ok(serde_json::to_value(memo.undo(id)?)?)
            }
            _ => Err(anyhow::anyhow!("unknown tool: {name}")),
        }
    };
    run().map_err(|error| format!("{error:#}"))
}

fn handle(memo: &mut Memo, message: &Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str);
    let response = |result: Value| json!({"jsonrpc":"2.0","id":id,"result":result});
    let error = |code: i64, message: &str| json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}});
    let Some(method) = method else {
        return Some(error(-32600, "invalid request"));
    };
    match method {
        "initialize" => Some(response(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "physis-replant", "version": env!("CARGO_PKG_VERSION")}
        }))),
        "notifications/initialized" => None,
        "ping" => Some(response(json!({}))),
        "tools/list" => Some(response(tool_list())),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(-32602, "missing tool name"));
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call(memo, name, &arguments) {
                Ok(value) => {
                    let is_error = value
                        .get("status")
                        .and_then(Value::as_str)
                        .is_some_and(|status| matches!(status, "failed" | "blocked"));
                    Some(response(text_result(&value, is_error)))
                }
                Err(reason) => Some(response(text_result(
                    &json!({"status":"failed","reason":reason}),
                    true,
                ))),
            }
        }
        _ => Some(error(-32601, "method not found")),
    }
}

/// Serve until stdin closes. Logs nothing to stdout except JSON-RPC responses.
pub fn serve(store: &Path, tools: ToolRegistry) -> Result<()> {
    let mut memo = Memo::open(store, tools)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(_) => {
                let response = json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}});
                serde_json::to_writer(&mut stdout, &response)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
                continue;
            }
        };
        if let Some(response) = handle(&mut memo, &message) {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::ToolRegistry;
    use serde_json::json;
    use tempfile::TempDir;

    fn memo(dir: &TempDir) -> Memo {
        Memo::open(dir.path().join("state"), ToolRegistry::new()).unwrap()
    }

    #[test]
    fn initialize_lists_tools_and_reports_a_clean_miss() {
        let dir = TempDir::new().unwrap();
        let mut memo = memo(&dir);
        let init = handle(
            &mut memo,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        let list = handle(
            &mut memo,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 7);
        let call = handle(&mut memo, &json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"replant_run","arguments":{"request":{"prompt":"never seen","input":{},"context":{}}}}
        })).unwrap();
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        let receipt: Value = serde_json::from_str(text).unwrap();
        assert_eq!(receipt["status"], "miss");
    }

    #[test]
    fn unknown_tools_and_methods_are_structured_errors() {
        let dir = TempDir::new().unwrap();
        let mut memo = memo(&dir);
        let method = handle(&mut memo, &json!({"jsonrpc":"2.0","id":1,"method":"nope"})).unwrap();
        assert_eq!(method["error"]["code"], -32601);
        let tool = handle(
            &mut memo,
            &json!({
                "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nope","arguments":{}}
            }),
        )
        .unwrap();
        assert_eq!(tool["result"]["isError"], true);
    }

    #[test]
    fn notifications_get_no_response() {
        let dir = TempDir::new().unwrap();
        let mut memo = memo(&dir);
        assert!(handle(
            &mut memo,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .is_none());
    }
}
