//! Minimal localhost/bridge HTTP bridge for n8n. No TLS; auth is a bearer token.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

use crate::workflow::{Memo, RecordInput, Request, ReviewInput, Status};

const MAX_HEADER: usize = 16_384;
const MAX_BODY: usize = 1 << 20;
const MAX_IN_FLIGHT: usize = 16;

pub struct ServerConfig {
    pub bind: SocketAddr,
    pub token: String,
}

struct HttpRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        if let Some(end) = find_header_end(&buffer) {
            break end;
        }
        ensure!(buffer.len() <= MAX_HEADER, "request headers exceed 16 KiB");
        let count = stream.read(&mut chunk).context("read request headers")?;
        ensure!(count > 0, "connection closed before headers");
        buffer.extend_from_slice(&chunk[..count]);
    };
    ensure!(header_end <= MAX_HEADER, "request headers exceed 16 KiB");
    let head = std::str::from_utf8(&buffer[..header_end]).context("headers are not UTF-8")?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().context("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("missing method")?.to_string();
    let path = parts.next().context("missing path")?.to_string();
    let version = parts.next().context("missing HTTP version")?;
    ensure!(
        version == "HTTP/1.1" || version == "HTTP/1.0",
        "unsupported HTTP version"
    );
    let mut authorization = None;
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "authorization" {
            authorization = Some(value.to_string());
        } else if name == "content-length" {
            content_length = Some(value.parse::<usize>().context("invalid Content-Length")?);
        }
    }
    let mut body = buffer[header_end + 4..].to_vec();
    if method == "POST" {
        let length = content_length.context("POST requires Content-Length")?;
        ensure!(length <= MAX_BODY, "request body exceeds 1 MiB");
        while body.len() < length {
            let count = stream.read(&mut chunk).context("read request body")?;
            ensure!(count > 0, "connection closed before body");
            body.extend_from_slice(&chunk[..count]);
        }
        body.truncate(length);
    } else {
        ensure!(body.is_empty(), "unexpected body on non-POST request");
    }
    Ok(HttpRequest {
        method,
        path,
        authorization,
        body,
    })
}

fn authorized(expected: &str, provided: Option<&str>) -> bool {
    let Some(provided) = provided.and_then(|value| value.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (expected.as_bytes(), provided.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn respond(stream: &mut TcpStream, status: u16, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Content",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}

fn dispatch(memo: &Mutex<Memo>, token: &str, request: HttpRequest) -> (u16, Value) {
    if request.path == "/healthz" {
        return if request.method == "GET" {
            (200, json!({"status":"ok"}))
        } else {
            (405, json!({"status":"failed","reason":"use GET /healthz"}))
        };
    }
    if !matches!(
        request.path.as_str(),
        "/run" | "/record" | "/approve" | "/reject" | "/undo" | "/stats"
    ) {
        return (404, json!({"status":"failed","reason":"unknown path"}));
    }
    if !authorized(token, request.authorization.as_deref()) {
        return (401, json!({"status":"failed","reason":"unauthorized"}));
    }
    let expected_method = if request.path == "/stats" {
        "GET"
    } else {
        "POST"
    };
    if request.method != expected_method {
        return (
            405,
            json!({"status":"failed","reason":format!("use {expected_method}")}),
        );
    }
    let mut memo = match memo.lock() {
        Ok(memo) => memo,
        Err(_) => {
            return (
                500,
                json!({"status":"failed","reason":"state lock poisoned"}),
            )
        }
    };
    if request.path == "/stats" {
        return match memo.stats() {
            Ok(stats) => (
                200,
                serde_json::to_value(stats).unwrap_or_else(|_| json!({"status":"failed"})),
            ),
            Err(error) => (
                500,
                json!({"status":"failed","reason":format!("{error:#}")}),
            ),
        };
    }
    let receipt = match request.path.as_str() {
        "/run" => serde_json::from_slice::<Request>(&request.body)
            .map_err(anyhow::Error::from)
            .and_then(|request| memo.run(&request)),
        "/record" => serde_json::from_slice::<RecordInput>(&request.body)
            .map_err(anyhow::Error::from)
            .and_then(|record| memo.record(&record)),
        "/approve" => serde_json::from_slice::<ReviewInput>(&request.body)
            .map_err(anyhow::Error::from)
            .and_then(|review| memo.approve(&review)),
        "/reject" => serde_json::from_slice::<ReviewInput>(&request.body)
            .map_err(anyhow::Error::from)
            .and_then(|review| memo.reject(&review)),
        _ => serde_json::from_slice::<serde_json::Value>(&request.body)
            .map_err(anyhow::Error::from)
            .and_then(|value| {
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("missing id"))
                    .and_then(|id| memo.undo(id))
            }),
    };
    match receipt {
        Ok(receipt) => {
            let status = match receipt.status {
                Status::Succeeded | Status::Miss | Status::Review | Status::Rejected => 200,
                Status::Blocked => 409,
                _ => 422,
            };
            (
                status,
                serde_json::to_value(receipt).unwrap_or_else(|_| json!({"status":"failed"})),
            )
        }
        Err(error) => (
            500,
            json!({"status":"failed","reason":format!("{error:#}")}),
        ),
    }
}

pub fn serve(config: ServerConfig, memo: Memo) -> Result<()> {
    ensure!(
        !config.bind.ip().is_unspecified(),
        "refusing to bind 0.0.0.0/::; name one interface"
    );
    ensure!(
        config.token.len() >= 16,
        "API token must be at least 16 characters"
    );
    let listener =
        TcpListener::bind(config.bind).with_context(|| format!("bind {}", config.bind))?;
    serve_listener(listener, Arc::new(Mutex::new(memo)), config.token)
}

fn serve_listener(listener: TcpListener, memo: Arc<Mutex<Memo>>, token: String) -> Result<()> {
    let in_flight = Arc::new(AtomicUsize::new(0));
    for connection in listener.incoming() {
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        if in_flight.load(Ordering::SeqCst) >= MAX_IN_FLIGHT {
            let _ = respond(
                &mut stream,
                503,
                &json!({"status":"failed","reason":"busy"}),
            );
            continue;
        }
        in_flight.fetch_add(1, Ordering::SeqCst);
        let memo = Arc::clone(&memo);
        let token = token.clone();
        let counter = Arc::clone(&in_flight);
        std::thread::spawn(move || {
            let result = read_request(&mut stream)
                .and_then(|request| Ok(dispatch(&memo, &token, request)))
                .and_then(|(status, value)| respond(&mut stream, status, &value));
            let _ = result;
            counter.fetch_sub(1, Ordering::SeqCst);
        });
    }
    bail!("listener stopped")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{ToolKind, ToolRegistry, ToolSpec};
    use serde_json::json;
    use std::io::Write as _;

    fn call(addr: SocketAddr, raw: &str) -> (u16, Value) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let (head, body) = reply.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, serde_json::from_str(body).unwrap())
    }

    fn fixture() -> (tempfile::TempDir, ToolRegistry) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("tools.py");
        std::fs::write(&script, "import json,sys,pathlib\nr=pathlib.Path(sys.argv[2])\nif sys.argv[1]=='check':\n print(json.dumps({'ok':True}))\nelse:\n c=r/'counter'\n n=int(c.read_text())+1 if c.exists() else 1\n c.write_text(str(n)); print(json.dumps({'ok':True,'n':n}))\n").unwrap();
        let spec = |kind: &str| ToolSpec {
            kind: if kind == "action" {
                ToolKind::Action
            } else {
                ToolKind::Check
            },
            program: "/usr/bin/python3".into(),
            args: vec![
                script.to_string_lossy().into(),
                kind.into(),
                dir.path().to_string_lossy().into(),
            ],
            fingerprint_files: vec![script.clone()],
            timeout_ms: 2000,
        };
        let mut tools = ToolRegistry::new();
        tools.insert("act".into(), spec("action"));
        tools.insert("check".into(), spec("check"));
        (dir, tools)
    }

    #[test]
    fn health_auth_routing_and_replay_over_http() {
        let (dir, tools) = fixture();
        let memo = Memo::open(dir.path().join("state"), tools).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let token = "0123456789abcdef".to_string();
        std::thread::spawn(move || serve_listener(listener, Arc::new(Mutex::new(memo)), token));

        let (status, body) = call(addr, "GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!((status, body["status"].as_str().unwrap()), (200, "ok"));

        let request = r#"{"prompt":"increment","input":{},"context":{}}"#;
        let raw = format!(
            "POST /run HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{}",
            request.len(),
            request
        );
        assert_eq!(call(addr, &raw).0, 401);

        let raw = format!(
            "POST /run HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer 0123456789abcdef\r\nContent-Length: {}\r\n\r\n{}",
            request.len(),
            request
        );
        let (status, body) = call(addr, &raw);
        assert_eq!(status, 200);
        assert_eq!(body["status"], "miss");

        let record = json!({
            "request": serde_json::from_str::<Value>(request).unwrap(),
            "recipe": {"steps":[{"tool":"act","input":{}}],"checks":[{"tool":"check","input":{}}]},
            "trace": {"provider":"http-test"}
        });
        let record = serde_json::to_string(&record).unwrap();
        let raw = format!(
            "POST /record HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer 0123456789abcdef\r\nContent-Length: {}\r\n\r\n{}",
            record.len(),
            record
        );
        let (status, body) = call(addr, &raw);
        assert_eq!(
            (status, body["status"].as_str().unwrap()),
            (200, "succeeded")
        );

        let raw = format!(
            "POST /run HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer 0123456789abcdef\r\nContent-Length: {}\r\n\r\n{}",
            request.len(),
            request
        );
        let (status, body) = call(addr, &raw);
        assert_eq!(
            (status, body["operation"].as_str().unwrap()),
            (200, "replay")
        );
        assert_eq!(body["provider_calls"], 0);
    }

    #[test]
    fn refuses_unspecified_bind() {
        let (dir, tools) = fixture();
        let memo = Memo::open(dir.path().join("state"), tools).unwrap();
        let config = ServerConfig {
            bind: "0.0.0.0:1".parse().unwrap(),
            token: "0123456789abcdef".into(),
        };
        assert!(serve(config, memo).is_err());
    }
}
