// SPDX-License-Identifier: Apache-2.0
//! Shared observation memory. Types are usable without the `store` feature.

pub mod observation;
pub use observation::{Observation, ObservationSource};
#[cfg(feature = "store")]
mod log;
#[cfg(feature = "store")]
pub mod redact;
#[cfg(feature = "store")]
pub use log::{load_observations, Memory, MemoryNote, MemoryPage, MemoryQuery};
