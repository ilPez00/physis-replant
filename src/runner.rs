//! Fixed operator-owned commands receive JSON on stdin. This is not a sandbox.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::workflow::{digest, ToolOutcome, ToolRegistry, ToolSpec};

pub(crate) const MAX_TOOL_BYTES: usize = 65_536;

pub(crate) fn validate_registry(tools: &ToolRegistry) -> Result<()> {
    ensure!(tools.len() <= 128, "registry exceeds 128 tools");
    for (name, spec) in tools {
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)),
            "invalid tool name"
        );
        ensure!(
            spec.program.is_absolute(),
            "tool program must be an absolute path"
        );
        ensure!(
            spec.args.len() <= 64 && spec.args.iter().map(|s| s.len()).sum::<usize>() <= 8192,
            "tool argv exceeds bounds"
        );
        ensure!(
            (1..=1_800_000).contains(&spec.timeout_ms),
            "timeout_ms must be in 1..=1800000"
        );
        ensure!(
            spec.fingerprint_files.len() <= 16
                && spec.fingerprint_files.iter().all(|p| p.is_absolute()),
            "fingerprint files must be at most 16 absolute paths"
        );
    }
    Ok(())
}

pub(crate) fn fingerprint(tools: &ToolRegistry) -> Result<String> {
    let mut fingerprints = BTreeMap::new();
    let mut bindings = Vec::new();
    for spec in tools.values() {
        for path in std::iter::once(&spec.program).chain(spec.fingerprint_files.iter()) {
            let canonical = path
                .canonicalize()
                .with_context(|| format!("missing tool implementation: {}", path.display()))?;
            bindings.push((path.clone(), canonical.clone()));
            if fingerprints.contains_key(&canonical) {
                continue;
            }
            let file = File::open(&canonical)?;
            ensure!(
                file.metadata()?.is_file() && file.metadata()?.len() <= 256 << 20,
                "tool fingerprint must be a regular file at most 256 MiB"
            );
            let mut hash = Sha256::new();
            let mut reader = file.take((256 << 20) + 1);
            let mut buffer = [0u8; 16_384];
            let mut total = 0;
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                total += count;
                ensure!(total <= 256 << 20, "tool fingerprint exceeds 256 MiB");
                hash.update(&buffer[..count]);
            }
            fingerprints.insert(canonical, format!("{:x}", hash.finalize()));
        }
    }
    // Compile-time source bytes pin the memorizer implementation as well as its
    // package version; an unreleased same-version code change also invalidates.
    digest(&(
        env!("CARGO_PKG_VERSION"),
        include_str!("workflow.rs"),
        include_str!("runner.rs"),
        include_str!("store.rs"),
        tools,
        bindings,
        fingerprints,
    ))
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
    error: Option<std::io::Error>,
}

fn capture(mut stream: impl Read, deadline: Instant, limit: usize, tail: bool) -> Capture {
    let mut kept = Capture::default();
    let mut buffer = [0u8; 4096];
    loop {
        if Instant::now() >= deadline {
            kept.error = Some(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tool output deadline exceeded",
            ));
            return kept;
        }
        let count = match stream.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                kept.error = Some(error);
                return kept;
            }
        };
        if count == 0 {
            return kept;
        }
        kept.truncated |= kept.bytes.len() + count > limit;
        if tail {
            kept.bytes.extend_from_slice(&buffer[..count]);
            if kept.bytes.len() > limit {
                kept.bytes.drain(..kept.bytes.len() - limit);
            }
        } else {
            let room = limit.saturating_sub(kept.bytes.len());
            kept.bytes.extend_from_slice(&buffer[..count.min(room)]);
        }
    }
}

#[cfg(unix)]
fn nonblocking(stream: &impl std::os::fd::AsRawFd) -> Result<()> {
    let fd = stream.as_raw_fd();
    // Borrowed pipe descriptors are valid for both fcntl calls; no ownership moves.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    ensure!(
        flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "cannot make tool pipe nonblocking"
    );
    Ok(())
}

fn send(mut stream: impl Write, bytes: &[u8], deadline: Instant) -> std::io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tool input deadline exceeded",
            ));
        }
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "tool input closed",
                ))
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn invoke(spec: &ToolSpec, input: &Value, outcome: &mut ToolOutcome) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let bytes = serde_json::to_vec(input)?;
    ensure!(bytes.len() <= MAX_TOOL_BYTES, "tool input exceeds 64 KiB");
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .context("cannot start registered tool")?;
    let pid = child.id();
    let stdin = child.stdin.take().context("tool stdin unavailable")?;
    let stdout = child.stdout.take().context("tool stdout unavailable")?;
    let stderr = child.stderr.take().context("tool stderr unavailable")?;
    let pipes = nonblocking(&stdin)
        .and_then(|_| nonblocking(&stdout))
        .and_then(|_| nonblocking(&stderr));
    if let Err(error) = pipes {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        let _ = child.wait();
        return Err(error);
    }
    let start = Instant::now();
    let deadline = start + Duration::from_millis(spec.timeout_ms);
    let writer = thread::spawn(move || send(stdin, &bytes, deadline));
    let output = thread::spawn(move || capture(stdout, deadline, MAX_TOOL_BYTES, false));
    let errors = thread::spawn(move || capture(stderr, deadline, 8192, true));
    let wait = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < Duration::from_millis(spec.timeout_ms) => {
                thread::sleep(Duration::from_millis(2))
            }
            Ok(None) => {
                break Err(anyhow::anyhow!(
                    "tool timed out after {} ms",
                    spec.timeout_ms
                ))
            }
            Err(e) => break Err(e.into()),
        }
    };
    // A tool must not leave background descendants holding pipes open. The group
    // was created for this child; negative pid addresses only that process group.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    let written = writer
        .join()
        .map_err(|_| anyhow::anyhow!("tool stdin thread failed"))?;
    let stdout = output
        .join()
        .map_err(|_| anyhow::anyhow!("tool stdout thread failed"))?;
    let stderr = errors
        .join()
        .map_err(|_| anyhow::anyhow!("tool stderr thread failed"))?;
    outcome.exit_code = wait.as_ref().ok().and_then(|status| status.code());
    outcome.stdout = bounded_text(&stdout.bytes, MAX_TOOL_BYTES);
    outcome.stderr = bounded_text(&stderr.bytes, 8192);
    outcome.stdout_truncated = stdout.truncated;
    outcome.stderr_truncated = stderr.truncated;
    let parsed = serde_json::from_slice::<Value>(&stdout.bytes);
    outcome.output = parsed.as_ref().ok().cloned();
    let status = wait?;
    written.context("tool did not accept JSON input")?;
    if let Some(error) = stdout.error {
        return Err(error.into());
    }
    if let Some(error) = stderr.error {
        return Err(error.into());
    }
    ensure!(
        !stdout.truncated && !stderr.truncated,
        "tool output exceeds capture bounds"
    );
    ensure!(status.success(), "tool exited unsuccessfully: {status}");
    let value = parsed.context("tool stdout is not one JSON object")?;
    ensure!(
        value.is_object() && value.get("ok").and_then(Value::as_bool) == Some(true),
        "tool did not report ok:true"
    );
    Ok(())
}

fn bounded_text(bytes: &[u8], limit: usize) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

#[cfg(not(unix))]
fn invoke(_spec: &ToolSpec, _input: &Value, _outcome: &mut ToolOutcome) -> Result<()> {
    anyhow::bail!("registered process execution currently requires Unix");
}

pub(crate) fn execute(name: &str, spec: &ToolSpec, input: &Value, phase: &str) -> ToolOutcome {
    let start = Instant::now();
    let mut outcome = ToolOutcome {
        tool: name.into(),
        kind: spec.kind,
        phase: phase.into(),
        ok: false,
        exit_code: None,
        output: None,
        error: None,
        elapsed_us: 0,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
    };
    match invoke(spec, input, &mut outcome) {
        Ok(()) => outcome.ok = true,
        Err(error) => outcome.error = Some(format!("{name}: {error:#}")),
    }
    outcome.elapsed_us = start.elapsed().as_micros();
    outcome
}
