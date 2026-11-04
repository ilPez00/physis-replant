// SPDX-License-Identifier: Apache-2.0
// Extracted from physis-next; standalone subset and imports. See NOTICE.
//! Trace helpers for replant.

use crate::ids::stable_id;
use crate::State;

/// Deterministic digest of a [`State`].
pub fn state_digest(state: &State) -> String {
    let metrics = serde_json::to_string(&state.metrics).unwrap_or_default();
    let refs = state.refs.join("\x00");
    stable_id(&[&state.description, &refs, &metrics])
}

/// Wall-clock millis since UNIX_EPOCH, as a string label.
pub fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn digest_changes_with_description() {
        let base = State {
            description: "a".into(),
            refs: Vec::new(),
            metrics: BTreeMap::new(),
        };
        let diff = State {
            description: "b".into(),
            refs: Vec::new(),
            metrics: BTreeMap::new(),
        };
        assert_ne!(state_digest(&base), state_digest(&diff));
    }

    #[test]
    fn digest_changes_with_refs() {
        let a = State {
            description: "a".into(),
            refs: vec!["x".into()],
            metrics: BTreeMap::new(),
        };
        let b = State {
            description: "a".into(),
            refs: vec!["y".into()],
            metrics: BTreeMap::new(),
        };
        assert_ne!(state_digest(&a), state_digest(&b));
    }
}
