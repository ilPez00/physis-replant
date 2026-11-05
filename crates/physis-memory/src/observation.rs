// SPDX-License-Identifier: Apache-2.0
// Extracted without schema changes from physis-next@64ae52fe4, physis-kernel.
//! Observation: what happened or was seen. Never a claim of truth.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObservationSource(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub seq: u64,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub source: ObservationSource,
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, String>,
}

impl Observation {
    pub fn new(source: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            seq: 0,
            at: Utc::now(),
            duration_ms: None,
            source: ObservationSource(source.into()),
            subject: subject.into(),
            body: String::new(),
            produced_by: None,
            props: BTreeMap::new(),
        }
    }
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
    pub fn lasting(mut self, duration_ms: u64) -> Self {
        self.duration_ms = if duration_ms > 0 {
            Some(duration_ms)
        } else {
            None
        };
        self
    }
    pub fn by(mut self, watcher: impl Into<String>) -> Self {
        self.produced_by = Some(watcher.into());
        self
    }
    pub fn prop(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.props.insert(key.into(), value.into());
        self
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.props.get(key).map(String::as_str)
    }
    pub fn overlaps(&self, other: &Observation) -> bool {
        let end = |o: &Observation| o.at.timestamp_millis() + o.duration_ms.unwrap_or(0) as i64;
        self.at.timestamp_millis() < end(other) && other.at.timestamp_millis() < end(self)
    }
}
