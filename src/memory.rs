//! General memory and redacted, idempotent receipt projections. Never replay authority.

use crate::{Memo, Receipt, RecordInput, Request, ReviewInput};
use anyhow::Result;
use physis_memory::{Memory, MemoryNote, MemoryPage, MemoryQuery, Observation};
use std::path::PathBuf;

impl Memo {
    /// Common native operation boundary for CLI/MCP/HTTP adapters. Never spawns another engine.
    pub fn dispatch(
        &mut self,
        operation: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value> {
        use serde_json::{from_value, json, to_value};
        if matches!(
            operation,
            "history" | "stats" | "sync-memory" | "memory-history"
        ) {
            anyhow::ensure!(input == json!({}), "operation accepts no input fields");
        }
        Ok(match operation {
            "run" => to_value(self.run(&from_value::<Request>(input)?)?)?,
            "record" => to_value(self.record(&from_value::<RecordInput>(input)?)?)?,
            "inspect" | "show" => to_value(self.inspect(&from_value::<Request>(input)?)?)?,
            "history" => to_value(self.history()?)?,
            "stats" => to_value(self.stats()?)?,
            "approve" => to_value(self.approve(&from_value::<ReviewInput>(input)?)?)?,
            "reject" => to_value(self.reject(&from_value::<ReviewInput>(input)?)?)?,
            "undo" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Undo {
                    id: String,
                }
                to_value(self.undo(&from_value::<Undo>(input)?.id)?)?
            }
            "remember" => to_value(self.remember(from_value(input)?)?)?,
            "memory-history" => to_value(self.memory_query(&MemoryQuery::default())?)?,
            "memory-search" => to_value(self.memory_query(&from_value(input)?)?)?,
            "sync-memory" => json!({"projected":self.sync_memory()?}),
            _ => anyhow::bail!("unknown Replant operation: {operation}"),
        })
    }

    /// Explicit project memory root. No daemon, environment read or graph dependency.
    pub fn with_memory_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.memory = Memory::new(root);
        self
    }
    pub fn remember(&self, note: MemoryNote) -> Result<Observation> {
        self.memory.remember(note)
    }
    pub fn memory_query(&self, query: &MemoryQuery) -> Result<MemoryPage> {
        self.memory.query(query)
    }

    /// Repair projections from validated journal evidence without running tools.
    pub fn sync_memory(&self) -> Result<usize> {
        // history() returns owned rows after dropping the execution-store lock.
        let history = self.history()?;
        if history.is_empty() {
            return Ok(0);
        }
        let store_id = self.store.identity()?;
        let mut rows = Vec::with_capacity(history.len());
        for receipt in history {
            let status = serde_json::to_value(receipt.status)?
                .as_str()
                .unwrap()
                .to_string();
            let operation = serde_json::to_value(receipt.operation)?
                .as_str()
                .unwrap()
                .to_string();
            let mut obs = Observation::new(
                "replant",
                format!("replant:{store_id}:{}:{status}", receipt.id),
            )
            .with_body("Execution journal projection; not authorization to execute.")
            .by("physis-replant")
            .prop("execution_store", &store_id)
            .prop("receipt_id", receipt.id)
            .prop("status", status)
            .prop("operation", operation)
            .prop("request_hash", receipt.key)
            .prop("policy_hash", receipt.policy_hash)
            .prop("engine_version", receipt.engine_version);
            if let Some(hash) = receipt.recipe_hash {
                obs = obs.prop("recipe_hash", hash);
            }
            rows.push(obs);
        }
        Ok(self.memory.append_unique(rows)?.len())
    }

    pub(crate) fn finish_operation(&self, result: Result<Receipt>) -> Result<Receipt> {
        let projection = self.sync_memory();
        match result {
            Ok(mut receipt) => {
                if let Err(error) = projection {
                    receipt.memory_warning = Some(format!("execution journal remains authoritative; memory projection failed: {error:#}; repair then sync-memory (do not rerun actions)"));
                }
                Ok(receipt)
            }
            Err(error) => match projection {
                Ok(_) => Err(error),
                Err(projection) => {
                    Err(error.context(format!("memory projection also failed: {projection:#}")))
                }
            },
        }
    }
}
