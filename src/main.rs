use anyhow::{bail, Context, Result};
use physis_replant::mcp;
use physis_replant::server::{serve, ServerConfig};
use physis_replant::{Memo, RecordInput, Request, ReviewInput, Status, ToolRegistry};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const MAX_INPUT: u64 = 1024 * 1024;
const HELP: &str =
    "replant <run|record|show|history|stats|approve|reject|undo|serve|mcp> [options]\n\
    run         Read {prompt,input,context}; replay, miss, or review candidates.\n\
    record      Read {request,recipe,trace}; execute, check and learn.\n\
    show        Read request JSON; return learned record or null.\n\
    history     Return receipt journal as a JSON array; no stdin.\n\
    stats       Return measured record/replay/review/undo counts; no stdin.\n\
    approve     Read {request,candidate}; execute a reviewed near-match and learn it.\n\
    reject      Read {request,candidate}; stop proposing that candidate for this request.\n\
    undo        Read {id}; run a learned record's declared undo steps in reverse.\n\
    serve       HTTP bridge for n8n: POST /run, /record; GET /healthz.\n\
    mcp         MCP stdio server (newline-delimited JSON-RPC 2.0).\n\
    --store     Private local state directory (default: .replant).\n\
    --tools     Operator-owned JSON tool registry (required except show/history/stats).\n\
    --bind      serve only: interface:port, e.g. 172.17.0.1:8799 (0.0.0.0 refused).\n\
    --token-file serve only: file containing the bearer token (mode 0600).\n\
                REPLANT_API_TOKEN is used when --token-file is omitted.\n\
    --help      Print this usage as JSON.\n\
    Stdin: exactly one JSON value, at most 1 MiB. Output: JSON only.\n\
    Exit 0: success, miss, review or help. Exit 1: rejected/failed operation.\n\
    No provider is called. Route miss/review to a planner or a human.";

struct Options {
    command: String,
    store: PathBuf,
    tools: Option<PathBuf>,
    bind: Option<String>,
    token_file: Option<PathBuf>,
    no_checks: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UndoInput {
    id: String,
}

fn options(args: Vec<OsString>) -> Result<Options> {
    let mut args = args.into_iter();
    let command = args.next().context("missing command; use --help")?;
    let command = command.to_str().context("command must be valid UTF-8")?;
    if !matches!(
        command,
        "run"
            | "record"
            | "show"
            | "history"
            | "stats"
            | "approve"
            | "reject"
            | "undo"
            | "serve"
            | "mcp"
    ) {
        bail!("unknown command: {command}; use --help");
    }
    let mut store = None;
    let mut tools = None;
    let mut bind = None;
    let mut token_file = None;
    let mut no_checks = false;
    while let Some(flag) = args.next() {
        let slot = match flag.to_str() {
            Some("--store") => &mut store,
            Some("--tools") => &mut tools,
            Some("--bind") => &mut bind,
            Some("--token-file") => &mut token_file,
            Some("--no-checks") => {
                no_checks = true;
                continue;
            }
            _ => bail!(
                "unexpected argument: {}; use --help",
                flag.to_string_lossy()
            ),
        };
        if slot.is_some() {
            bail!("duplicate option: {}", flag.to_string_lossy());
        }
        let value = args
            .next()
            .with_context(|| format!("missing path for {}", flag.to_string_lossy()))?;
        if value.is_empty() || value.to_string_lossy().starts_with('-') {
            bail!("expected path after {}", flag.to_string_lossy());
        }
        *slot = Some(PathBuf::from(value));
    }
    if matches!(
        command,
        "run" | "record" | "approve" | "reject" | "undo" | "serve" | "mcp"
    ) && tools.is_none()
    {
        bail!("--tools FILE is required for {command}");
    }
    if command != "serve" && (bind.is_some() || token_file.is_some()) {
        bail!("--bind and --token-file are only valid for serve");
    }
    Ok(Options {
        command: command.into(),
        store: store.unwrap_or_else(|| PathBuf::from(".replant")),
        tools,
        bind: bind.map(|path| path.to_string_lossy().into_owned()),
        token_file,
        no_checks,
    })
}

fn input<T: DeserializeOwned>() -> Result<T> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)
        .context("read stdin")?;
    if bytes.len() as u64 > MAX_INPUT {
        bail!("stdin exceeds 1 MiB limit");
    }
    serde_json::from_slice(&bytes).context("stdin must contain exactly one valid JSON value")
}

fn read_token(path: Option<&PathBuf>) -> Result<String> {
    let token = match path {
        Some(path) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path)?.permissions().mode();
                if mode & 0o077 != 0 {
                    bail!("token file must be private (mode 0600)");
                }
            }
            std::fs::read_to_string(path)?.trim().to_string()
        }
        None => std::env::var("REPLANT_API_TOKEN")
            .context("serve needs --token-file or REPLANT_API_TOKEN (at least 16 characters)")?,
    };
    if token.len() < 16 {
        bail!("API token must be at least 16 characters");
    }
    Ok(token)
}

fn execute(args: Vec<OsString>) -> Result<(Value, bool)> {
    if args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return Ok((json!({"usage": HELP}), true));
    }
    let options = options(args)?;
    // Validate stdin before opening or changing persistent state.
    let request: Option<Request> = if matches!(options.command.as_str(), "run" | "show") {
        Some(input()?)
    } else {
        None
    };
    let record: Option<RecordInput> = if options.command == "record" {
        Some(input()?)
    } else {
        None
    };
    let review: Option<ReviewInput> = if matches!(options.command.as_str(), "approve" | "reject") {
        Some(input()?)
    } else {
        None
    };
    let undo: Option<UndoInput> = if options.command == "undo" {
        Some(input()?)
    } else {
        None
    };
    let tools: ToolRegistry = match options.tools {
        Some(path) => {
            let file = File::open(&path)
                .with_context(|| format!("open tool registry {}", path.display()))?;
            let mut bytes = Vec::new();
            file.take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_INPUT {
                bail!("tool registry exceeds 1 MiB");
            }
            serde_json::from_slice(&bytes).context("invalid tool registry JSON")?
        }
        None => ToolRegistry::new(),
    };
    if options.command == "mcp" {
        mcp::serve(&options.store, tools)?;
        return Ok((json!({"status":"stopped"}), true));
    }
    let mut memo = Memo::open(&options.store, tools)?;
    if options.command == "serve" {
        let bind = options
            .bind
            .as_deref()
            .context("serve requires --bind interface:port")?;
        let bind = bind.parse().context("--bind must be IP:PORT")?;
        let token = read_token(options.token_file.as_ref())?;
        serve(ServerConfig { bind, token }, memo)?;
        return Ok((json!({"status":"stopped"}), true));
    }
    let receipt = match options.command.as_str() {
        "run" => memo.run(request.as_ref().context("missing request")?, options.no_checks)?,
        "record" => memo.record(record.as_ref().context("missing record")?)?,
        "approve" => memo.approve(review.as_ref().context("missing review input")?)?,
        "reject" => memo.reject(review.as_ref().context("missing review input")?)?,
        "undo" => memo.undo(&undo.context("missing undo input")?.id)?,
        "show" => {
            return Ok((
                serde_json::to_value(memo.inspect(request.as_ref().context("missing request")?)?)?,
                true,
            ));
        }
        "history" => return Ok((serde_json::to_value(memo.history()?)?, true)),
        "stats" => return Ok((serde_json::to_value(memo.stats()?)?, true)),
        _ => unreachable!("validated command"),
    };
    let success = matches!(
        &receipt.status,
        Status::Succeeded | Status::Miss | Status::Review | Status::Rejected
    );
    Ok((serde_json::to_value(receipt)?, success))
}

fn main() -> ExitCode {
    let (value, success) = match execute(std::env::args_os().skip(1).collect()) {
        Ok(result) => result,
        Err(error) => (
            json!({"status":"failed", "reason":format!("{error:#}")}),
            false,
        ),
    };
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &value).is_err() || writeln!(stdout).is_err() {
        return ExitCode::FAILURE;
    }
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
