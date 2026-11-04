// SPDX-License-Identifier: Apache-2.0
// Extracted from physis-next; standalone subset. See NOTICE.
//! Stable string ids. Entities are never identified by embeddings.

use sha2::{Digest, Sha256};

/// Short stable hash for derived addresses.
pub fn stable_id(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    format!("{:x}", h.finalize())[..16].to_string()
}
