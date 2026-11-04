//! Private atomic records and a fail-closed journal, serialized by an OS lock.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::{de::DeserializeOwned, Serialize};

use crate::workflow::{LearnedRecord, Operation, Receipt, Status, MAX_JSON_BYTES};

const MAX_JOURNAL_BYTES: u64 = 16 << 20;

pub(crate) struct Store {
    root: PathBuf,
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

fn private_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "state directories must not be symlinks"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "state directory must be private (mode 0700): {}",
            path.display()
        );
    }
    Ok(())
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid state identifier"
    );
    Ok(())
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        private_dir(root)?;
        let root = root.canonicalize()?;
        private_dir(&root.join("entries"))?;
        private_dir(&root.join("records"))?;
        Ok(Self { root })
    }

    pub fn lock(&self) -> Result<File> {
        let file = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("lock"))?;
        file.lock().context("cannot lock state directory")?;
        Ok(file) // Closing the descriptor releases its OS lock, including on crash.
    }

    pub fn history(&self) -> Result<Vec<Receipt>> {
        let path = self.root.join("journal.jsonl");
        let mut file = match private_options().read(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            file.metadata()?.len() <= MAX_JOURNAL_BYTES,
            "journal exceeds 16 MiB; archive state explicitly"
        );
        let mut body = String::new();
        Read::by_ref(&mut file)
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_string(&mut body)?;
        ensure!(
            body.len() as u64 <= MAX_JOURNAL_BYTES,
            "journal exceeds 16 MiB"
        );
        ensure!(
            body.is_empty() || body.ends_with('\n'),
            "journal has a torn tail; refusing to discard crash evidence"
        );
        let mut history = Vec::new();
        let mut states: BTreeMap<String, Receipt> = BTreeMap::new();
        for (line, row) in body.lines().enumerate() {
            let receipt: Receipt = serde_json::from_str(row)
                .with_context(|| format!("corrupt journal at line {}", line + 1))?;
            identifier(&receipt.id)?;
            identifier(&receipt.key)?;
            if let Some(prior) = states.get(&receipt.id) {
                ensure!(
                    prior.status == Status::Started
                        && matches!(
                            receipt.status,
                            Status::Succeeded | Status::Failed | Status::Blocked
                        ),
                    "invalid receipt lifecycle"
                );
                ensure!(
                    prior.key == receipt.key
                        && prior.operation == receipt.operation
                        && prior.policy_hash == receipt.policy_hash
                        && prior.recipe_hash == receipt.recipe_hash,
                    "receipt identity changed during execution"
                );
            } else {
                ensure!(
                    receipt.status == Status::Started
                        || (receipt.operation == Operation::Lookup
                            && matches!(receipt.status, Status::Miss | Status::Blocked)),
                    "terminal receipt lacks a start"
                );
            }
            states.insert(receipt.id.clone(), receipt.clone());
            history.push(receipt);
        }
        Ok(history)
    }

    pub fn append(&self, receipt: &Receipt) -> Result<()> {
        let mut row = serde_json::to_vec(receipt)?;
        row.push(b'\n');
        let mut file = private_options()
            .append(true)
            .create(true)
            .open(self.root.join("journal.jsonl"))?;
        // shortcut: bounded single journal, explicit archive is needed at 16 MiB.
        ensure!(
            file.metadata()?.len() + row.len() as u64 <= MAX_JOURNAL_BYTES,
            "journal exceeds 16 MiB; archive state explicitly"
        );
        file.write_all(&row)?;
        file.sync_all()?;
        Ok(())
    }

    fn read<T: DeserializeOwned>(&self, path: &Path) -> Result<Option<T>> {
        let mut file = match private_options().read(true).open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            file.metadata()?.len() <= (MAX_JSON_BYTES as u64 + 8192),
            "state record is too large"
        );
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_JSON_BYTES as u64 + 8193)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_JSON_BYTES + 8192,
            "state record is too large"
        );
        Ok(Some(
            serde_json::from_slice(&bytes).context("corrupt state record")?,
        ))
    }

    fn write<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        let temporary = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            crate::trace::now()
        ));
        let mut file = private_options()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().context("state path has no parent")?)?.sync_all()?;
        Ok(())
    }

    pub fn save_record(&self, record: &LearnedRecord) -> Result<()> {
        identifier(&record.id)?;
        let path = self
            .root
            .join("records")
            .join(format!("{}.json", record.id));
        ensure!(!path.try_exists()?, "record id collision");
        self.write(&path, record)
    }

    pub fn index(&self, key: &str, id: &str) -> Result<()> {
        identifier(key)?;
        identifier(id)?;
        self.write(&self.root.join("entries").join(format!("{key}.json")), &id)
    }

    pub fn entry(&self, key: &str) -> Result<Option<LearnedRecord>> {
        identifier(key)?;
        let id: Option<String> =
            self.read(&self.root.join("entries").join(format!("{key}.json")))?;
        let Some(id) = id else { return Ok(None) };
        identifier(&id)?;
        let record: LearnedRecord = self
            .read(&self.root.join("records").join(format!("{id}.json")))?
            .context("index points at a missing record")?;
        if record.id != id {
            bail!("record identity does not match its index");
        }
        Ok(Some(record))
    }
}
