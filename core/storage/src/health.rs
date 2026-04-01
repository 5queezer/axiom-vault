//! Provider health tracking for multi-backend storage.
//!
//! Monitors the availability of each backend with success/failure counts,
//! latency metrics, and automatic state transitions (Healthy → Degraded → Offline).
//! Degraded backends are skipped for reads but still attempted for writes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Health status of a single storage backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Backend is operating normally.
    Healthy,
    /// Backend has exceeded the failure threshold; skipped for reads.
    Degraded,
    /// Backend has exceeded the offline threshold; skipped for reads.
    Offline,
}

/// Configuration for health tracking behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Consecutive failures before marking a backend as degraded (default: 3).
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    /// Consecutive failures before marking a backend as offline (default: 10).
    #[serde(default = "default_offline_threshold")]
    pub offline_threshold: u32,
    /// Seconds to wait before attempting a recovery probe on a degraded/offline
    /// backend (default: 60).
    #[serde(default = "default_recovery_interval_secs")]
    pub recovery_interval_secs: u64,
}

fn default_failure_threshold() -> u32 {
    3
}
fn default_offline_threshold() -> u32 {
    10
}
fn default_recovery_interval_secs() -> u64 {
    60
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_failure_threshold(),
            offline_threshold: default_offline_threshold(),
            recovery_interval_secs: default_recovery_interval_secs(),
        }
    }
}

impl HealthConfig {
    /// Recovery interval as a `Duration`.
    pub fn recovery_interval(&self) -> Duration {
        Duration::from_secs(self.recovery_interval_secs)
    }
}

/// Per-backend health state.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    /// Current health status.
    pub status: HealthStatus,
    /// Number of consecutive failures (resets to 0 on success).
    pub consecutive_failures: u32,
    /// Timestamp of the last successful operation.
    pub last_success: Option<DateTime<Utc>>,
    /// Timestamp of the last failed operation.
    pub last_failure: Option<DateTime<Utc>>,
    /// Timestamp of the last recovery probe attempt.
    pub last_probe: Option<DateTime<Utc>>,
    /// Exponentially weighted moving average of operation latency in milliseconds.
    pub avg_latency_ms: f64,
    /// Backend index (immutable, for logging).
    pub backend_index: usize,
}

const EWMA_ALPHA: f64 = 0.3;

impl ProviderHealth {
    /// Create a new health state for the given backend index, starting as `Healthy`.
    pub fn new(backend_index: usize) -> Self {
        Self {
            status: HealthStatus::Healthy,
            consecutive_failures: 0,
            last_success: None,
            last_failure: None,
            last_probe: None,
            avg_latency_ms: 0.0,
            backend_index,
        }
    }

    /// Record a successful operation. Resets failure count and restores `Healthy` status.
    pub fn record_success(&mut self, latency: Duration) {
        self.consecutive_failures = 0;
        self.status = HealthStatus::Healthy;
        self.last_success = Some(Utc::now());

        let latency_ms = latency.as_secs_f64() * 1000.0;
        if self.avg_latency_ms == 0.0 {
            self.avg_latency_ms = latency_ms;
        } else {
            self.avg_latency_ms =
                EWMA_ALPHA * latency_ms + (1.0 - EWMA_ALPHA) * self.avg_latency_ms;
        }
    }

    /// Record a failed operation. May transition to `Degraded` or `Offline`.
    pub fn record_failure(&mut self, config: &HealthConfig) {
        self.consecutive_failures += 1;
        self.last_failure = Some(Utc::now());

        if self.consecutive_failures >= config.offline_threshold {
            self.status = HealthStatus::Offline;
        } else if self.consecutive_failures >= config.failure_threshold {
            self.status = HealthStatus::Degraded;
        }
    }

    /// Whether this backend should be skipped for read operations.
    pub fn should_skip_for_reads(&self) -> bool {
        matches!(self.status, HealthStatus::Degraded | HealthStatus::Offline)
    }

    /// Whether this backend is eligible for a recovery probe.
    ///
    /// Returns `true` if the backend is not `Healthy` and enough time has elapsed
    /// since the last failure or probe.
    pub fn should_probe(&self, config: &HealthConfig) -> bool {
        if self.status == HealthStatus::Healthy {
            return false;
        }

        let reference = match (self.last_probe, self.last_failure) {
            (Some(p), Some(f)) => Some(p.max(f)),
            (Some(p), None) => Some(p),
            (None, Some(f)) => Some(f),
            (None, None) => return true,
        };

        match reference {
            Some(t) => {
                let elapsed = Utc::now().signed_duration_since(t);
                elapsed
                    .to_std()
                    .map(|d| d >= config.recovery_interval())
                    .unwrap_or(true)
            }
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_health_config() {
        let config = HealthConfig::default();
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.offline_threshold, 10);
        assert_eq!(config.recovery_interval_secs, 60);
        assert_eq!(config.recovery_interval(), Duration::from_secs(60));
    }

    #[test]
    fn test_health_config_serde_roundtrip() {
        let config = HealthConfig {
            failure_threshold: 5,
            offline_threshold: 15,
            recovery_interval_secs: 120,
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: HealthConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.failure_threshold, 5);
        assert_eq!(decoded.offline_threshold, 15);
        assert_eq!(decoded.recovery_interval_secs, 120);
    }

    #[test]
    fn test_health_config_serde_defaults() {
        // Empty JSON should use defaults
        let decoded: HealthConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(decoded.failure_threshold, 3);
        assert_eq!(decoded.offline_threshold, 10);
        assert_eq!(decoded.recovery_interval_secs, 60);
    }

    #[test]
    fn test_health_status_serde_roundtrip() {
        for status in [
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Offline,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: HealthStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, status);
        }
    }

    #[test]
    fn test_new_provider_health_is_healthy() {
        let h = ProviderHealth::new(0);
        assert_eq!(h.status, HealthStatus::Healthy);
        assert_eq!(h.consecutive_failures, 0);
        assert!(h.last_success.is_none());
        assert!(h.last_failure.is_none());
        assert!(h.last_probe.is_none());
        assert_eq!(h.avg_latency_ms, 0.0);
        assert_eq!(h.backend_index, 0);
    }

    #[test]
    fn test_record_success_resets_to_healthy() {
        let config = HealthConfig::default();
        let mut h = ProviderHealth::new(0);

        // Drive to degraded
        for _ in 0..config.failure_threshold {
            h.record_failure(&config);
        }
        assert_eq!(h.status, HealthStatus::Degraded);

        // One success resets
        h.record_success(Duration::from_millis(50));
        assert_eq!(h.status, HealthStatus::Healthy);
        assert_eq!(h.consecutive_failures, 0);
        assert!(h.last_success.is_some());
    }

    #[test]
    fn test_record_success_from_offline() {
        let config = HealthConfig::default();
        let mut h = ProviderHealth::new(0);

        for _ in 0..config.offline_threshold {
            h.record_failure(&config);
        }
        assert_eq!(h.status, HealthStatus::Offline);

        h.record_success(Duration::from_millis(10));
        assert_eq!(h.status, HealthStatus::Healthy);
        assert_eq!(h.consecutive_failures, 0);
    }

    #[test]
    fn test_transition_healthy_to_degraded() {
        let config = HealthConfig::default();
        let mut h = ProviderHealth::new(0);

        for i in 0..config.failure_threshold {
            if i < config.failure_threshold - 1 {
                assert_eq!(h.status, HealthStatus::Healthy);
            }
            h.record_failure(&config);
        }
        assert_eq!(h.status, HealthStatus::Degraded);
        assert_eq!(h.consecutive_failures, config.failure_threshold);
    }

    #[test]
    fn test_transition_degraded_to_offline() {
        let config = HealthConfig::default();
        let mut h = ProviderHealth::new(0);

        for _ in 0..config.offline_threshold {
            h.record_failure(&config);
        }
        assert_eq!(h.status, HealthStatus::Offline);
        assert_eq!(h.consecutive_failures, config.offline_threshold);
    }

    #[test]
    fn test_should_skip_for_reads() {
        let config = HealthConfig::default();
        let mut h = ProviderHealth::new(0);

        assert!(!h.should_skip_for_reads());

        for _ in 0..config.failure_threshold {
            h.record_failure(&config);
        }
        assert!(h.should_skip_for_reads()); // Degraded

        for _ in config.failure_threshold..config.offline_threshold {
            h.record_failure(&config);
        }
        assert!(h.should_skip_for_reads()); // Offline
    }

    #[test]
    fn test_should_probe_healthy_returns_false() {
        let config = HealthConfig::default();
        let h = ProviderHealth::new(0);
        assert!(!h.should_probe(&config));
    }

    #[test]
    fn test_should_probe_degraded_no_timestamps_returns_true() {
        let config = HealthConfig::default();
        let mut h = ProviderHealth::new(0);
        h.status = HealthStatus::Degraded;
        h.last_failure = None;
        h.last_probe = None;
        assert!(h.should_probe(&config));
    }

    #[test]
    fn test_should_probe_respects_recovery_interval() {
        let config = HealthConfig {
            recovery_interval_secs: 3600, // 1 hour
            ..HealthConfig::default()
        };
        let mut h = ProviderHealth::new(0);
        h.status = HealthStatus::Degraded;
        h.last_failure = Some(Utc::now()); // Just failed

        // Should NOT probe — recovery interval hasn't elapsed
        assert!(!h.should_probe(&config));
    }

    #[test]
    fn test_should_probe_after_interval_elapsed() {
        let config = HealthConfig {
            recovery_interval_secs: 1,
            ..HealthConfig::default()
        };
        let mut h = ProviderHealth::new(0);
        h.status = HealthStatus::Degraded;
        // Failed 2 seconds ago
        h.last_failure = Some(Utc::now() - chrono::Duration::seconds(2));

        assert!(h.should_probe(&config));
    }

    #[test]
    fn test_latency_ewma_first_sample() {
        let mut h = ProviderHealth::new(0);
        h.record_success(Duration::from_millis(100));
        assert!((h.avg_latency_ms - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_latency_ewma_updates() {
        let mut h = ProviderHealth::new(0);
        h.record_success(Duration::from_millis(100));
        h.record_success(Duration::from_millis(200));
        // EWMA: 0.3 * 200 + 0.7 * 100 = 60 + 70 = 130
        assert!((h.avg_latency_ms - 130.0).abs() < 0.01);
    }

    #[test]
    fn test_intermittent_failures_reset() {
        let config = HealthConfig::default();
        let mut h = ProviderHealth::new(0);

        // 2 failures (below threshold)
        h.record_failure(&config);
        h.record_failure(&config);
        assert_eq!(h.status, HealthStatus::Healthy);

        // Success resets counter
        h.record_success(Duration::from_millis(10));
        assert_eq!(h.consecutive_failures, 0);

        // 2 more failures — still healthy because counter was reset
        h.record_failure(&config);
        h.record_failure(&config);
        assert_eq!(h.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_last_probe_updates_probe_window() {
        let config = HealthConfig {
            recovery_interval_secs: 3600,
            ..HealthConfig::default()
        };
        let mut h = ProviderHealth::new(0);
        h.status = HealthStatus::Degraded;
        // Failed long ago
        h.last_failure = Some(Utc::now() - chrono::Duration::seconds(7200));
        // But probed just now
        h.last_probe = Some(Utc::now());

        // Should NOT probe — last_probe is recent
        assert!(!h.should_probe(&config));
    }
}
