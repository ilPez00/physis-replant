//! Verified recipes and versioned decisions. Model traces are metadata, not proof.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{runner, store::Store};

pub const STRATEGY: &str = "exact-request-verified-tools-v1";
pub const MAX_JSON_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub prompt: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub context: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Action,
    Check,
}

fn default_timeout() -> u64 {
    30_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    pub kind: ToolKind,
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Declare script/config implementation dependencies, never credentials.
    #[serde(default)]
    pub fingerprint_files: Vec<PathBuf>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

pub type ToolRegistry = BTreeMap<String, ToolSpec>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    #[serde(default)]
    pub preconditions: Vec<ToolCall>,
    pub steps: Vec<ToolCall>,
    pub checks: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordInput {
    pub request: Request,
    pub recipe: Recipe,
    pub trace: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Record,
    Replay,
    Lookup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Started,
    Succeeded,
    Failed,
    Miss,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub tool: String,
    pub kind: ToolKind,
    pub phase: String,
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub elapsed_us: u128,
}

/// One append-only memorizer decision; Started and terminal rows share an id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub id: String,
    pub key: String,
    pub operation: Operation,
    pub status: Status,
    pub engine_version: String,
    pub strategy: String,
    pub policy_hash: String,
    pub recipe_hash: Option<String>,
    pub provider_calls: u64,
    pub elapsed_us: u128,
    pub calls: Vec<ToolOutcome>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedRecord {
    pub schema: u32,
    pub id: String,
    pub request: Request,
    pub recipe: Recipe,
    pub trace: Value,
    pub policy_hash: String,
    pub recipe_hash: String,
    pub engine_version: String,
    pub strategy: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    pub executions: u64,
    pub verified_records: u64,
    pub verified_replays: u64,
    pub failures: u64,
    pub misses: u64,
    pub blocked: u64,
    pub pending: u64,
    pub replay_share_percent: f64,
}

/// The store is private local state, not an authorization or multi-tenant server.
pub struct Memo {
    store: Store,
    tools: ToolRegistry,
}

pub(crate) fn digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

fn request_key(request: &Request) -> Result<String> {
    ensure!(
        !request.prompt.trim().is_empty(),
        "prompt must not be empty"
    );
    ensure!(
        serde_json::to_vec(request)?.len() <= MAX_JSON_BYTES,
        "request exceeds 1 MiB"
    );
    digest(&(STRATEGY, request))
}

impl Memo {
    pub fn open(path: impl AsRef<Path>, tools: ToolRegistry) -> Result<Self> {
        runner::validate_registry(&tools)?;
        Ok(Self {
            store: Store::open(path.as_ref())?,
            tools,
        })
    }

    pub fn run(&mut self, request: &Request) -> Result<Receipt> {
        let key = request_key(request)?;
        let _lock = self.store.lock()?;
        let history = self.store.history()?;
        let policy = runner::fingerprint(&self.tools)?;
        let last_execution = history
            .iter()
            .rev()
            .find(|r| r.key == key && r.operation != Operation::Lookup);
        let entry = self.store.entry(&key)?;
        if last_execution.is_some_and(|r| r.status != Status::Succeeded)
            || (last_execution.is_some() && entry.is_none())
        {
            let receipt = self.receipt(
                &key,
                Operation::Lookup,
                Status::Blocked,
                policy,
                None,
                history.len(),
            )?;
            let receipt = Receipt { reason: Some("prior execution failed, was interrupted, or lacks its index; inspect effects before recording again".into()), ..receipt };
            self.store.append(&receipt)?;
            return Ok(receipt);
        }
        let reason = if let Some(entry) = &entry {
            ensure!(
                entry.request == *request,
                "stored request does not match its key"
            );
            ensure!(
                digest(&entry.recipe)? == entry.recipe_hash,
                "stored recipe hash mismatch"
            );
            ensure!(entry.schema == 1, "unsupported record schema");
            if last_execution.is_some_and(|r| r.operation == Operation::Record && r.id != entry.id)
            {
                bail!(
                    "latest verified record was not indexed; inspect state before recording again"
                );
            }
            if entry.policy_hash != policy
                || entry.engine_version != env!("CARGO_PKG_VERSION")
                || entry.strategy != STRATEGY
            {
                Some("tool implementation, registry or engine changed")
            } else if !history.iter().any(|r| {
                r.id == entry.id
                    && r.key == key
                    && r.operation == Operation::Record
                    && r.status == Status::Succeeded
                    && r.recipe_hash.as_deref() == Some(entry.recipe_hash.as_str())
                    && r.policy_hash == entry.policy_hash
            }) {
                bail!("record lacks its successful verification receipt");
            } else {
                None
            }
        } else {
            Some("no verified recipe for this exact request")
        };
        if let Some(reason) = reason {
            let receipt = self.receipt(
                &key,
                Operation::Lookup,
                Status::Miss,
                policy,
                None,
                history.len(),
            )?;
            let receipt = Receipt {
                reason: Some(reason.into()),
                ..receipt
            };
            self.store.append(&receipt)?;
            return Ok(receipt);
        }
        let entry = entry.context("missing verified record")?;
        self.validate_recipe(&entry.recipe)?;
        let receipt = self.receipt(
            &key,
            Operation::Replay,
            Status::Started,
            policy,
            Some(entry.recipe_hash),
            history.len(),
        )?;
        self.execute(receipt, &entry.recipe)
    }

    /// Record after executing independent registered checks, never confidence alone.
    pub fn record(&mut self, input: &RecordInput) -> Result<Receipt> {
        let key = request_key(&input.request)?;
        ensure!(
            serde_json::to_vec(input)?.len() <= MAX_JSON_BYTES,
            "record exceeds 1 MiB"
        );
        ensure!(
            input
                .trace
                .as_object()
                .is_some_and(|object| !object.is_empty()),
            "trace must be a nonempty JSON object"
        );
        self.validate_recipe(&input.recipe)?;
        let _lock = self.store.lock()?;
        let history = self.store.history()?;
        let policy = runner::fingerprint(&self.tools)?;
        let recipe_hash = digest(&input.recipe)?;
        let receipt = self.receipt(
            &key,
            Operation::Record,
            Status::Started,
            policy.clone(),
            Some(recipe_hash.clone()),
            history.len(),
        )?;
        let learned = LearnedRecord {
            schema: 1,
            id: receipt.id.clone(),
            request: input.request.clone(),
            recipe: input.recipe.clone(),
            trace: input.trace.clone(),
            policy_hash: policy,
            recipe_hash,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            strategy: STRATEGY.into(),
        };
        self.store.save_record(&learned)?;
        let receipt = self.execute(receipt, &input.recipe)?;
        if receipt.status == Status::Succeeded {
            self.store.index(&key, &learned.id)?;
        }
        Ok(receipt)
    }

    pub fn inspect(&self, request: &Request) -> Result<Option<LearnedRecord>> {
        let key = request_key(request)?;
        let _lock = self.store.lock()?;
        self.store.history()?;
        self.store.entry(&key)
    }

    pub fn history(&self) -> Result<Vec<Receipt>> {
        let _lock = self.store.lock()?;
        self.store.history()
    }

    pub fn stats(&self) -> Result<Stats> {
        let mut latest = BTreeMap::new();
        for receipt in self.history()? {
            latest.insert(receipt.id.clone(), receipt);
        }
        let mut stats = Stats::default();
        for receipt in latest.values() {
            match (receipt.operation, receipt.status) {
                (_, Status::Started) => {
                    stats.pending += 1;
                    stats.executions += 1;
                }
                (Operation::Lookup, Status::Miss) => stats.misses += 1,
                (Operation::Lookup, Status::Blocked) => stats.blocked += 1,
                (Operation::Record, Status::Succeeded) => {
                    stats.verified_records += 1;
                    stats.executions += 1;
                }
                (Operation::Replay, Status::Succeeded) => {
                    stats.verified_replays += 1;
                    stats.executions += 1;
                }
                (_, Status::Failed | Status::Blocked) => {
                    stats.failures += 1;
                    stats.executions += 1;
                }
                _ => bail!("invalid receipt state in statistics"),
            }
        }
        if stats.executions > 0 {
            stats.replay_share_percent =
                100.0 * stats.verified_replays as f64 / stats.executions as f64;
        }
        Ok(stats)
    }

    fn validate_recipe(&self, recipe: &Recipe) -> Result<()> {
        ensure!(
            !recipe.steps.is_empty() && !recipe.checks.is_empty(),
            "recipe requires actions and independent checks"
        );
        ensure!(
            recipe.steps.len() + recipe.checks.len() + recipe.preconditions.len() <= 32,
            "recipe exceeds 32 calls"
        );
        for (calls, kind) in [
            (&recipe.steps, ToolKind::Action),
            (&recipe.checks, ToolKind::Check),
            (&recipe.preconditions, ToolKind::Check),
        ] {
            for call in calls {
                let spec = self
                    .tools
                    .get(&call.tool)
                    .with_context(|| format!("unknown tool: {}", call.tool))?;
                ensure!(
                    spec.kind == kind,
                    "tool {} is not registered as {kind:?}",
                    call.tool
                );
                ensure!(
                    serde_json::to_vec(&call.input)?.len() <= runner::MAX_TOOL_BYTES,
                    "tool input exceeds 64 KiB"
                );
            }
        }
        Ok(())
    }

    fn receipt(
        &self,
        key: &str,
        operation: Operation,
        status: Status,
        policy_hash: String,
        recipe_hash: Option<String>,
        seq: usize,
    ) -> Result<Receipt> {
        let id = digest(&(key, seq, std::process::id(), crate::trace::now()))?;
        Ok(Receipt {
            id,
            key: key.into(),
            operation,
            status,
            policy_hash,
            recipe_hash,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            strategy: STRATEGY.into(),
            provider_calls: 0,
            elapsed_us: 0,
            calls: Vec::new(),
            reason: None,
        })
    }

    fn execute(&self, mut receipt: Receipt, recipe: &Recipe) -> Result<Receipt> {
        // Persist intent before effects: an interrupted invocation cannot look clean.
        self.store.append(&receipt)?;
        let start = Instant::now();
        'phases: for (phase, calls) in [
            ("precondition", &recipe.preconditions),
            ("action", &recipe.steps),
            ("check", &recipe.checks),
        ] {
            for call in calls {
                if let Err(error) = self.verify_policy(&receipt.policy_hash) {
                    receipt.reason = Some(format!("{error:#}"));
                    break 'phases;
                }
                let outcome =
                    runner::execute(&call.tool, &self.tools[&call.tool], &call.input, phase);
                let ok = outcome.ok;
                if !ok {
                    receipt.reason = outcome.error.clone();
                }
                receipt.calls.push(outcome);
                if !ok {
                    break 'phases;
                }
            }
        }
        if receipt.reason.is_none() {
            if let Err(error) = self.verify_policy(&receipt.policy_hash) {
                receipt.reason = Some(format!("{error:#}"));
            }
        }
        receipt.elapsed_us = start.elapsed().as_micros();
        receipt.status = if receipt.reason.is_some() {
            Status::Failed
        } else {
            Status::Succeeded
        };
        self.store.append(&receipt)?;
        Ok(receipt)
    }

    fn verify_policy(&self, expected: &str) -> Result<()> {
        ensure!(
            runner::fingerprint(&self.tools)? == expected,
            "tool implementation changed during execution; inspect effects before recording again"
        );
        Ok(())
    }
}
