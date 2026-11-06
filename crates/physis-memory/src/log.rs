// SPDX-License-Identifier: Apache-2.0
// Observation append/locking and parallel-prefix parsing adapted from
// physis-next@64ae52fe4, crates/physis-store/src/log.rs.
//! One shared observation log, independent of graph/event/runtime storage.

use crate::{redact, Observation};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryNote {
    pub subject: String,
    pub text: String,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryQuery {
    pub query: String,
    pub limit: usize,
}
impl Default for MemoryQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 20,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryPage {
    pub observations: Vec<Observation>,
    pub matched: usize,
    pub truncated: bool,
    pub degraded: bool,
}

#[derive(Debug, Clone)]
pub struct Memory {
    root: PathBuf,
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

impl Memory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn path(&self) -> PathBuf {
        self.root.join("observations.jsonl")
    }

    fn guard(&self) -> Result<File> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.root)?;
        let metadata = std::fs::symlink_metadata(&self.root)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "memory root must be a directory, not a symlink"
        );
        let file = options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("observations.lock"))?;
        let start = std::time::Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) => {
                    ensure!(
                        start.elapsed() < std::time::Duration::from_secs(10),
                        "observation log lock busy for 10 seconds"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    fn update(
        &self,
        make: impl FnOnce(&[Observation]) -> Result<Vec<Observation>>,
    ) -> Result<Vec<Observation>> {
        let _guard = self.guard()?;
        let (existing, degraded) = load_observations(&self.path());
        ensure!(!degraded, "observation log is corrupt, unreadable or has a torn tail; append refused without deleting evidence");
        let mut seq = match existing.last() {
            Some(last) => last
                .seq
                .checked_add(1)
                .context("observation sequence exhausted")?,
            None => 0,
        };
        let mut items = make(&existing)?;
        if items.is_empty() {
            return Ok(items);
        }
        let mut file = options().append(true).create(true).open(self.path())?;
        for obs in &mut items {
            redact::scrub(obs);
            obs.seq = seq;
            seq = seq
                .checked_add(1)
                .context("observation sequence exhausted")?;
            let line = serde_json::to_vec(obs)?;
            file.write_all(&line)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        Ok(items)
    }

    pub fn append(&self, obs: Observation) -> Result<Observation> {
        self.append_if(obs, |_| Ok(true))?
            .context("unconditional append was rejected")
    }
    pub fn append_if(
        &self,
        obs: Observation,
        condition: impl FnOnce(&[Observation]) -> Result<bool>,
    ) -> Result<Option<Observation>> {
        Ok(self
            .update(|existing| {
                Ok(if condition(existing)? {
                    vec![obs]
                } else {
                    vec![]
                })
            })?
            .pop())
    }
    pub fn append_batch(
        &self,
        items: impl IntoIterator<Item = Observation>,
    ) -> Result<Vec<Observation>> {
        self.update(|_| Ok(items.into_iter().collect()))
    }
    /// Lifecycle projections identify themselves by source + subject; atomic across writers.
    pub fn append_unique(
        &self,
        items: impl IntoIterator<Item = Observation>,
    ) -> Result<Vec<Observation>> {
        self.update(|existing| {
            let mut seen: BTreeSet<_> = existing
                .iter()
                .map(|o| (o.source.0.clone(), o.subject.clone()))
                .collect();
            Ok(items
                .into_iter()
                .filter(|o| seen.insert((o.source.0.clone(), o.subject.clone())))
                .collect())
        })
    }
    pub fn remember(&self, note: MemoryNote) -> Result<Observation> {
        ensure!(
            !note.subject.trim().is_empty() && !note.text.trim().is_empty(),
            "note needs nonempty subject and text"
        );
        ensure!(
            serde_json::to_vec(&note)?.len() <= 1 << 20,
            "note exceeds 1 MiB"
        );
        let mut obs = Observation::new("memory", note.subject).with_body(note.text);
        obs.produced_by = note.actor;
        self.append(obs)
    }
    pub fn query(&self, query: &MemoryQuery) -> Result<MemoryPage> {
        ensure!(
            (1..=1000).contains(&query.limit) && query.query.len() <= 65536,
            "memory query limit must be 1..1000 and text at most 64 KiB"
        );
        let (rows, degraded) = load_observations(&self.path());
        let text = query.query.to_lowercase();
        let matching: Vec<_> = rows
            .into_iter()
            .filter(|row| {
                text.is_empty()
                    || row.subject.to_lowercase().contains(&text)
                    || row.body.to_lowercase().contains(&text)
            })
            .collect();
        let matched = matching.len();
        let mut observations = Vec::new();
        let mut bytes = 0;
        for row in matching.into_iter().rev().take(query.limit) {
            bytes += serde_json::to_vec(&row)?.len();
            if bytes > 1 << 20 {
                break;
            }
            observations.push(row);
        }
        observations.reverse();
        Ok(MemoryPage {
            truncated: observations.len() < matched,
            observations,
            matched,
            degraded,
        })
    }
}

/// Read the valid prefix without concealing corruption, IO errors or unsafe sequence order.
pub fn load_observations(path: &Path) -> (Vec<Observation>, bool) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return (vec![], error.kind() != std::io::ErrorKind::NotFound),
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let target = bytes.len().div_ceil(threads).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let want = (start + target).min(bytes.len());
        let end = bytes[want..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|offset| want + offset + 1)
            .unwrap_or(bytes.len());
        chunks.push(&bytes[start..end]);
        start = end;
    }
    let parts = std::thread::scope(|scope| {
        chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    let mut items = Vec::new();
                    for line in chunk.split(|b| *b == b'\n') {
                        if line.iter().all(u8::is_ascii_whitespace) {
                            continue;
                        }
                        match serde_json::from_slice::<Observation>(line) {
                            Ok(item) => items.push(item),
                            Err(_) => return (items, true),
                        }
                    }
                    (items, false)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().expect("observation parser panicked"))
            .collect::<Vec<_>>()
    });
    let mut out: Vec<Observation> = Vec::new();
    let mut sequence_break = false;
    for (items, damaged) in parts {
        for item in items {
            if out.last().is_some_and(|last| item.seq <= last.seq) {
                // Keep parsed evidence: Next's invariant checker must still see the break.
                sequence_break = true;
            }
            out.push(item);
        }
        if damaged {
            return (out, true);
        }
    }
    (
        out,
        sequence_break || (!bytes.is_empty() && !bytes.ends_with(b"\n")),
    )
}
