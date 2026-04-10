//! Health tracking methods for the composite storage provider.

use tracing::warn;

use crate::health::{HealthStatus, ProviderHealth};
use axiomvault_common::VaultPath;

use super::config::RaidMode;
use super::CompositeStorageProvider;

impl CompositeStorageProvider {
    /// Get a snapshot of a backend's health state.
    pub async fn backend_health(&self, index: usize) -> Option<ProviderHealth> {
        match self.health_states.get(index) {
            Some(hs) => Some(hs.read().await.clone()),
            None => None,
        }
    }

    /// Get the number of backends currently in `Healthy` status.
    pub async fn healthy_backend_count(&self) -> usize {
        let mut count = 0;
        for hs in &self.health_states {
            if hs.read().await.status == HealthStatus::Healthy {
                count += 1;
            }
        }
        count
    }

    /// Indices of backends that are not degraded/offline.
    /// Falls back to all backends if none are healthy (prevents total lockout).
    pub(crate) async fn healthy_backend_indices(&self) -> Vec<usize> {
        let mut indices = Vec::new();
        for (i, hs) in self.health_states.iter().enumerate() {
            if !hs.read().await.should_skip_for_reads() {
                indices.push(i);
            }
        }
        if indices.is_empty() {
            (0..self.backends.len()).collect()
        } else {
            indices
        }
    }

    /// Attempt a lightweight recovery probe on backends that are due for one.
    pub(crate) async fn probe_if_due(&self) {
        for (i, hs) in self.health_states.iter().enumerate() {
            // Claim the probe window under write lock *before* the network call
            // to prevent concurrent reads from firing duplicate probes.
            let should = {
                let mut state = hs.write().await;
                if state.should_probe(&self.config.health) {
                    state.last_probe = Some(chrono::Utc::now());
                    true
                } else {
                    false
                }
            };
            if should {
                let start = tokio::time::Instant::now();
                let result = self.backends[i].exists(&VaultPath::root()).await;
                let latency = start.elapsed();
                let mut state = hs.write().await;
                match result {
                    Ok(_) => {
                        tracing::info!(
                            backend = self.backends[i].name(),
                            index = i,
                            "Backend recovered after probe"
                        );
                        state.record_success(latency);
                    }
                    Err(_) => {
                        // Still down — last_probe was already claimed above
                    }
                }
            }
        }
    }

    /// Log a warning if the number of healthy backends drops below a safe threshold.
    pub(crate) async fn check_redundancy_warning(&self) {
        let healthy = self.healthy_backend_count().await;
        let total = self.backends.len();

        let min_safe = match self.config.mode {
            RaidMode::Mirror => 2,
            RaidMode::Erasure { data_shards, .. } => data_shards + 1,
        };

        if healthy < min_safe {
            warn!(
                healthy_backends = healthy,
                total_backends = total,
                min_safe = min_safe,
                "Redundancy below safe threshold — risk of data loss on further failures"
            );
        }
    }

    /// Record a success on the given backend's health state.
    pub(crate) async fn record_health_success(&self, index: usize, latency: std::time::Duration) {
        if let Some(hs) = self.health_states.get(index) {
            hs.write().await.record_success(latency);
        }
    }

    /// Record a failure on the given backend's health state.
    pub(crate) async fn record_health_failure(&self, index: usize) {
        if let Some(hs) = self.health_states.get(index) {
            hs.write().await.record_failure(&self.config.health);
        }
    }
}
