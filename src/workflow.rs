//! Verified recipes, near-match review, compensation, and versioned decisions.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{runner, store::Store};

pub const STRATEGY: &str = "exact-request-verified-tools-v1";
pub const MAX_JSON_BYTES: usize = 1 << 20;
/// Near-match proposals are opt-in: never auto-replayed, always reviewed.
pub const REVIEW_THRESHOLD: f64 = 0.80;
pub const MAX_REVIEW_CANDIDATES: usize = 3;

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
    Undo,
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
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    pub preconditions: Vec<ToolCall>,
    pub steps: Vec<ToolCall>,
    pub checks: Vec<ToolCall>,
    /// Declared inverse of `steps`; run on failure or by explicit `undo`.
    pub undo: Vec<ToolCall>,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            preconditions: Vec::new(),
            steps: Vec::new(),
            checks: Vec::new(),
            undo: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordInput {
    pub request: Request,
    pub recipe: Recipe,
    pub trace: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewInput {
    pub request: Request,
    pub candidate: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Record,
    Rerecord,
    Replay,
    Lookup,
    Approve,
    Reject,
    Undo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Started,
    Succeeded,
    Failed,
    Miss,
    Stale,
    Blocked,
    Review,
    Rejected,
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
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewCandidate {
    pub id: String,
    pub similarity: f64,
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleRecord {
    pub id: String,
    pub recipe: Recipe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RerecordInput {
    pub id: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ReviewCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_record: Option<StaleRecord>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rejection {
    pub key: String,
    pub candidate: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    pub executions: u64,
    pub verified_records: u64,
    pub verified_replays: u64,
    pub failures: u64,
    pub misses: u64,
    #[serde(default)]
    pub stale: u64,
    pub reviews: u64,
    pub rejected: u64,
    pub undos: u64,
    pub undo_failures: u64,
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

fn tokens(request: &Request) -> BTreeSet<String> {
    let mut text = request.prompt.to_lowercase();
    text.push(' ');
    text.push_str(&request.input.to_string());
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Token Jaccard over prompt+input. Context equality is required separately.
pub fn similarity(a: &Request, b: &Request) -> f64 {
    let (a, b) = (tokens(a), tokens(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection = a.intersection(&b).count() as f64;
    let union = a.union(&b).count() as f64;
    intersection / union
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
        let last_execution = history.iter().rev().find(|r| {
            r.key == key && r.operation != Operation::Lookup && r.operation != Operation::Reject
        });
        let entry = self.store.entry(&key)?;
        if last_execution.is_some_and(|r| r.status != Status::Succeeded)
            || (last_execution.is_some() && entry.is_none())
            || last_execution.is_some_and(|r| r.operation == Operation::Undo)
        {
            let receipt = self.receipt(
                &key,
                Operation::Lookup,
                Status::Blocked,
                policy,
                None,
                history.len(),
            )?;
            let receipt = Receipt { reason: Some("prior execution failed, was interrupted, was undone, or lacks its index; inspect effects before recording again".into()), ..receipt };
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
            if last_execution.is_some_and(|r| {
                matches!(
                    r.operation,
                    Operation::Record | Operation::Approve | Operation::Rerecord
                ) && r.id != entry.id
            }) {
                bail!(
                    "latest verified record was not indexed; inspect state before recording again"
                );
            }
            ensure!(
                self.verified_source(entry, &history)?,
                "record lacks a current index and successful original verification receipt"
            );
            if entry.policy_hash != policy
                || entry.engine_version != env!("CARGO_PKG_VERSION")
                || entry.strategy != STRATEGY
            {
                Some("tool implementation, registry or engine changed")
            } else {
                None
            }
        } else {
            Some("no verified recipe for this exact request")
        };
        if let Some(reason) = reason {
            let candidates = if entry.is_none() {
                self.review_candidates(request, &key, &history, &policy)?
            } else {
                Vec::new()
            };
            let (status, reason) = if entry.is_some() {
                (Status::Stale, reason)
            } else if candidates.is_empty() {
                (Status::Miss, reason)
            } else {
                (
                    Status::Review,
                    "near-match recipes require explicit approval",
                )
            };
            let receipt =
                self.receipt(&key, Operation::Lookup, status, policy, None, history.len())?;
            let receipt = Receipt {
                reason: Some(reason.into()),
                candidates,
                stale_record: entry.as_ref().map(|entry| StaleRecord {
                    id: entry.id.clone(),
                    recipe: entry.recipe.clone(),
                }),
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

    /// Explicitly verify the indexed stored request/template under the current registry.
    /// This never changes request or context and never retries a failed history.
    pub fn rerecord(&mut self, id: &str) -> Result<Receipt> {
        let _lock = self.store.lock()?;
        let source = self.store.record(id)?.context("unknown record")?;
        let history = self.store.history()?;
        ensure!(
            self.verified_source(&source, &history)?,
            "source is not a current, indexed, successfully verified record"
        );
        self.validate_recipe(&source.recipe)?;
        let key = request_key(&source.request)?;
        let policy = runner::fingerprint(&self.tools)?;
        let receipt = self.receipt(
            &key,
            Operation::Rerecord,
            Status::Started,
            policy.clone(),
            Some(source.recipe_hash.clone()),
            history.len(),
        )?;
        let learned = LearnedRecord {
            id: receipt.id.clone(),
            policy_hash: policy,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            strategy: STRATEGY.into(),
            ..source
        };
        self.store.save_record(&learned)?;
        let receipt = self.execute(receipt, &learned.recipe)?;
        if receipt.status == Status::Succeeded {
            self.store.index(&key, &learned.id)?;
        }
        Ok(receipt)
    }

    /// Approve a near-match candidate: execute its recipe under this exact request.
    pub fn approve(&mut self, input: &ReviewInput) -> Result<Receipt> {
        let key = request_key(&input.request)?;
        let _lock = self.store.lock()?;
        let history = self.store.history()?;
        let policy = runner::fingerprint(&self.tools)?;
        if self.store.entry(&key)?.is_some() {
            bail!("an exact verified recipe already exists; use run");
        }
        if history.iter().rev().any(|r| {
            r.key == key
                && r.operation != Operation::Lookup
                && r.operation != Operation::Reject
                && r.status != Status::Succeeded
        }) {
            bail!("prior failed execution for this request; inspect effects before approving");
        }
        let candidate = self
            .store
            .record(&input.candidate)?
            .context("unknown candidate record")?;
        ensure!(
            self.verified_candidate(&candidate, &history, &policy)?,
            "candidate is not a current, indexed, verified recipe"
        );
        ensure!(
            candidate.request.context == input.request.context,
            "candidate context does not match; refusing cross-context approval"
        );
        let score = similarity(&input.request, &candidate.request);
        ensure!(
            score >= REVIEW_THRESHOLD,
            "candidate similarity {score:.3} is below {REVIEW_THRESHOLD:.2}"
        );
        self.validate_recipe(&candidate.recipe)?;
        let recipe_hash = digest(&candidate.recipe)?;
        let receipt = self.receipt(
            &key,
            Operation::Approve,
            Status::Started,
            policy.clone(),
            Some(recipe_hash.clone()),
            history.len(),
        )?;
        let learned = LearnedRecord {
            schema: 1,
            id: receipt.id.clone(),
            request: input.request.clone(),
            recipe: candidate.recipe.clone(),
            trace: candidate.trace.clone(),
            policy_hash: policy,
            recipe_hash,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            strategy: STRATEGY.into(),
        };
        self.store.save_record(&learned)?;
        let receipt = self.execute(receipt, &learned.recipe)?;
        if receipt.status == Status::Succeeded {
            self.store.index(&key, &learned.id)?;
        }
        Ok(receipt)
    }

    pub fn reject(&mut self, input: &ReviewInput) -> Result<Receipt> {
        let key = request_key(&input.request)?;
        let _lock = self.store.lock()?;
        let history = self.store.history()?;
        let policy = runner::fingerprint(&self.tools)?;
        self.store
            .record(&input.candidate)?
            .context("unknown candidate record")?;
        self.store.append_rejection(&Rejection {
            key: key.clone(),
            candidate: input.candidate.clone(),
        })?;
        let receipt = self.receipt(
            &key,
            Operation::Reject,
            Status::Rejected,
            policy,
            None,
            history.len(),
        )?;
        self.store.append(&receipt)?;
        Ok(receipt)
    }

    /// Explicitly run a record's declared undo steps in reverse order.
    pub fn undo(&mut self, id: &str) -> Result<Receipt> {
        let _lock = self.store.lock()?;
        let record = self.store.record(id)?.context("unknown record")?;
        ensure!(
            !record.recipe.undo.is_empty(),
            "recipe declares no undo steps"
        );
        self.validate_undo(&record.recipe)?;
        let key = request_key(&record.request)?;
        let history = self.store.history()?;
        let policy = runner::fingerprint(&self.tools)?;
        let receipt = self.receipt(
            &key,
            Operation::Undo,
            Status::Started,
            policy,
            Some(record.recipe_hash),
            history.len(),
        )?;
        self.store.append(&receipt)?;
        let start = Instant::now();
        let mut receipt = receipt;
        for call in record.recipe.undo.iter().rev() {
            if let Err(error) = self.verify_policy(&receipt.policy_hash) {
                receipt.reason = Some(format!("{error:#}"));
                break;
            }
            let outcome = runner::execute(&call.tool, &self.tools[&call.tool], &call.input, "undo");
            let ok = outcome.ok;
            if !ok {
                receipt.reason = outcome
                    .error
                    .clone()
                    .map(|e| format!("undo step failed: {e}"));
            }
            receipt.calls.push(outcome);
            if !ok {
                break;
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
                (Operation::Lookup, Status::Stale) => stats.stale += 1,
                (Operation::Lookup, Status::Blocked) => stats.blocked += 1,
                (Operation::Lookup, Status::Review) => stats.reviews += 1,
                (Operation::Reject, Status::Rejected) => stats.rejected += 1,
                (
                    Operation::Record | Operation::Approve | Operation::Rerecord,
                    Status::Succeeded,
                ) => {
                    stats.verified_records += 1;
                    stats.executions += 1;
                }
                (Operation::Replay, Status::Succeeded) => {
                    stats.verified_replays += 1;
                    stats.executions += 1;
                }
                (Operation::Undo, Status::Succeeded) => {
                    stats.undos += 1;
                    stats.executions += 1;
                }
                (Operation::Undo, Status::Failed) => {
                    stats.undo_failures += 1;
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

    fn verified_candidate(
        &self,
        record: &LearnedRecord,
        history: &[Receipt],
        policy: &str,
    ) -> Result<bool> {
        if record.schema != 1
            || record.policy_hash != policy
            || record.engine_version != env!("CARGO_PKG_VERSION")
            || record.strategy != STRATEGY
        {
            return Ok(false);
        }
        self.verified_source(record, history)
    }

    fn verified_source(&self, record: &LearnedRecord, history: &[Receipt]) -> Result<bool> {
        if record.schema != 1 || digest(&record.recipe)? != record.recipe_hash {
            return Ok(false);
        }
        let key = request_key(&record.request)?;
        if !self
            .store
            .entry(&key)?
            .is_some_and(|entry| entry.id == record.id)
        {
            return Ok(false);
        }
        let latest = history.iter().rev().find(|r| {
            r.key == key && !matches!(r.operation, Operation::Lookup | Operation::Reject)
        });
        Ok(latest.is_some_and(|r| {
            r.status == Status::Succeeded
                && r.operation != Operation::Undo
                && (!matches!(
                    r.operation,
                    Operation::Record | Operation::Approve | Operation::Rerecord
                ) || r.id == record.id)
        }) && history.iter().any(|r| {
            r.id == record.id
                && r.key == key
                && matches!(
                    r.operation,
                    Operation::Record | Operation::Approve | Operation::Rerecord
                )
                && r.status == Status::Succeeded
                && r.recipe_hash.as_deref() == Some(record.recipe_hash.as_str())
                && r.policy_hash == record.policy_hash
                && r.engine_version == record.engine_version
                && r.strategy == record.strategy
        }))
    }

    fn review_candidates(
        &self,
        request: &Request,
        key: &str,
        history: &[Receipt],
        policy: &str,
    ) -> Result<Vec<ReviewCandidate>> {
        let rejected: BTreeSet<String> = self
            .store
            .rejections()?
            .into_iter()
            .filter(|r| r.key == key)
            .map(|r| r.candidate)
            .collect();
        let mut candidates = Vec::new();
        for record in self.store.records()? {
            if record.request.context != request.context
                || rejected.contains(&record.id)
                || !self.verified_candidate(&record, history, policy)?
            {
                continue;
            }
            let score = similarity(request, &record.request);
            if score >= REVIEW_THRESHOLD {
                candidates.push(ReviewCandidate {
                    id: record.id,
                    similarity: score,
                    request: record.request,
                });
            }
        }
        candidates.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        candidates.truncate(MAX_REVIEW_CANDIDATES);
        Ok(candidates)
    }

    fn validate_recipe(&self, recipe: &Recipe) -> Result<()> {
        ensure!(
            !recipe.steps.is_empty() && !recipe.checks.is_empty(),
            "recipe requires actions and independent checks"
        );
        ensure!(
            recipe.steps.len()
                + recipe.checks.len()
                + recipe.preconditions.len()
                + recipe.undo.len()
                <= 32,
            "recipe exceeds 32 calls"
        );
        for (calls, kind) in [
            (&recipe.steps, ToolKind::Action),
            (&recipe.checks, ToolKind::Check),
            (&recipe.preconditions, ToolKind::Check),
            (&recipe.undo, ToolKind::Undo),
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

    fn validate_undo(&self, recipe: &Recipe) -> Result<()> {
        for call in &recipe.undo {
            let spec = self
                .tools
                .get(&call.tool)
                .with_context(|| format!("unknown undo tool: {}", call.tool))?;
            ensure!(
                spec.kind == ToolKind::Undo,
                "tool {} is not registered as Undo",
                call.tool
            );
            ensure!(
                serde_json::to_vec(&call.input)?.len() <= runner::MAX_TOOL_BYTES,
                "undo input exceeds 64 KiB"
            );
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
            candidates: Vec::new(),
            stale_record: None,
            reason: None,
        })
    }

    fn execute(&self, mut receipt: Receipt, recipe: &Recipe) -> Result<Receipt> {
        // Persist intent before effects: an interrupted invocation cannot look clean.
        self.store.append(&receipt)?;
        let start = Instant::now();
        let mut actions_ran = false;
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
                if phase == "action" {
                    actions_ran = true;
                }
                if !ok {
                    receipt.reason = outcome.error.clone();
                }
                receipt.calls.push(outcome);
                if !ok {
                    break 'phases;
                }
            }
        }
        if receipt.reason.is_some() && actions_ran && !recipe.undo.is_empty() {
            for call in recipe.undo.iter().rev() {
                if let Err(error) = self.verify_policy(&receipt.policy_hash) {
                    receipt.reason = Some(format!(
                        "{}; compensation failed: {error:#}",
                        receipt.reason.unwrap_or_default()
                    ));
                    break;
                }
                let outcome =
                    runner::execute(&call.tool, &self.tools[&call.tool], &call.input, "undo");
                let ok = outcome.ok;
                if !ok {
                    let error = outcome.error.clone().unwrap_or_default();
                    receipt.reason = Some(format!(
                        "{}; compensation failed: {error}",
                        receipt.reason.unwrap_or_default()
                    ));
                }
                receipt.calls.push(outcome);
                if !ok {
                    break;
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
