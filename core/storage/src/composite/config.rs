//! Configuration types for the composite storage provider.

use serde::{Deserialize, Serialize};

use crate::health::HealthConfig;

/// RAID mode for the composite provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaidMode {
    /// Mirror (RAID 1): write all chunks to all backends, read from first success.
    Mirror,
    /// Erasure coding (RAID 5/6): Reed-Solomon sharding across backends.
    Erasure {
        data_shards: usize,
        parity_shards: usize,
    },
}

/// Configuration for the composite storage provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositeConfig {
    /// RAID mode to use.
    pub mode: RaidMode,
    /// Health tracking configuration.
    #[serde(default)]
    pub health: HealthConfig,
}
