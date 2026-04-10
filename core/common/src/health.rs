//! Unified health check types for AxiomVault.
//!
//! Provides shared abstractions for health reporting across vault integrity
//! checks and storage provider availability tracking. Both subsystems produce
//! [`DiagnosticResult`] items collected into a [`HealthReport`].

use serde::{Deserialize, Serialize};

/// Overall health status of a component.
///
/// Used by both storage backends (runtime availability) and vault integrity
/// checks (structural correctness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Component is operating normally.
    Healthy,
    /// Component has issues that may need attention but is still partially functional.
    Degraded,
    /// Component is unavailable or has critical errors.
    #[serde(alias = "Offline")]
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "Healthy"),
            HealthStatus::Degraded => write!(f, "Degraded"),
            HealthStatus::Unhealthy => write!(f, "Unhealthy"),
        }
    }
}

/// Severity level for a single diagnostic finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// Informational finding, no action needed.
    Info,
    /// Potential problem that may need attention.
    Warning,
    /// Definite problem that affects correctness or availability.
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "Info"),
            Severity::Warning => write!(f, "Warning"),
            Severity::Error => write!(f, "Error"),
        }
    }
}

/// A single diagnostic finding from a health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticResult {
    /// Name of the check that produced this result.
    pub check_name: String,
    /// Severity of the finding.
    pub severity: Severity,
    /// Human-readable description of the finding.
    pub message: String,
    /// Whether this issue can be automatically fixed.
    pub auto_fixable: bool,
}

/// Complete health report for a component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Identifier for the component that was checked (e.g. vault path, backend name).
    pub component: String,
    /// Overall health status, derived from the individual results.
    pub status: HealthStatus,
    /// Individual diagnostic results.
    pub results: Vec<DiagnosticResult>,
}

impl HealthReport {
    /// Create a new report, computing the overall status from the results.
    ///
    /// - Any `Severity::Error` → `Unhealthy`
    /// - Any `Severity::Warning` (without errors) → `Degraded`
    /// - All `Severity::Info` → `Healthy`
    pub fn new(component: impl Into<String>, results: Vec<DiagnosticResult>) -> Self {
        let status = Self::derive_status(&results);
        Self {
            component: component.into(),
            status,
            results,
        }
    }

    /// Derive overall status from a set of diagnostic results.
    fn derive_status(results: &[DiagnosticResult]) -> HealthStatus {
        let has_errors = results
            .iter()
            .any(|r| matches!(r.severity, Severity::Error));
        let has_warnings = results
            .iter()
            .any(|r| matches!(r.severity, Severity::Warning));

        if has_errors {
            HealthStatus::Unhealthy
        } else if has_warnings {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }

    /// Returns `true` if any diagnostic result has `Severity::Error`.
    pub fn has_errors(&self) -> bool {
        self.results
            .iter()
            .any(|r| matches!(r.severity, Severity::Error))
    }

    /// Returns `true` if any diagnostic result has `Severity::Warning`.
    pub fn has_warnings(&self) -> bool {
        self.results
            .iter()
            .any(|r| matches!(r.severity, Severity::Warning))
    }

    /// Serialize the report to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "Healthy");
        assert_eq!(HealthStatus::Degraded.to_string(), "Degraded");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "Unhealthy");
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(Severity::Info.to_string(), "Info");
        assert_eq!(Severity::Warning.to_string(), "Warning");
        assert_eq!(Severity::Error.to_string(), "Error");
    }

    #[test]
    fn test_health_report_all_info_is_healthy() {
        let report = HealthReport::new(
            "test",
            vec![DiagnosticResult {
                check_name: "check1".to_string(),
                severity: Severity::Info,
                message: "All good".to_string(),
                auto_fixable: false,
            }],
        );
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(!report.has_errors());
        assert!(!report.has_warnings());
    }

    #[test]
    fn test_health_report_warning_is_degraded() {
        let report = HealthReport::new(
            "test",
            vec![DiagnosticResult {
                check_name: "check1".to_string(),
                severity: Severity::Warning,
                message: "Something off".to_string(),
                auto_fixable: false,
            }],
        );
        assert_eq!(report.status, HealthStatus::Degraded);
        assert!(!report.has_errors());
        assert!(report.has_warnings());
    }

    #[test]
    fn test_health_report_error_is_unhealthy() {
        let report = HealthReport::new(
            "test",
            vec![DiagnosticResult {
                check_name: "check1".to_string(),
                severity: Severity::Error,
                message: "Broken".to_string(),
                auto_fixable: false,
            }],
        );
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(report.has_errors());
    }

    #[test]
    fn test_health_report_error_trumps_warning() {
        let report = HealthReport::new(
            "test",
            vec![
                DiagnosticResult {
                    check_name: "check1".to_string(),
                    severity: Severity::Warning,
                    message: "Minor".to_string(),
                    auto_fixable: false,
                },
                DiagnosticResult {
                    check_name: "check2".to_string(),
                    severity: Severity::Error,
                    message: "Major".to_string(),
                    auto_fixable: false,
                },
            ],
        );
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_report_empty_results_is_healthy() {
        let report = HealthReport::new("test", vec![]);
        assert_eq!(report.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_health_report_json_roundtrip() {
        let report = HealthReport::new(
            "test-component",
            vec![DiagnosticResult {
                check_name: "check1".to_string(),
                severity: Severity::Info,
                message: "All good".to_string(),
                auto_fixable: false,
            }],
        );
        let json = report.to_json().unwrap();
        let decoded: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.component, "test-component");
        assert_eq!(decoded.status, HealthStatus::Healthy);
        assert_eq!(decoded.results.len(), 1);
    }

    #[test]
    fn test_health_status_serde_roundtrip() {
        for status in [
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Unhealthy,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: HealthStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, status);
        }
    }

    #[test]
    fn test_health_status_deserializes_legacy_offline() {
        let json = r#""Offline""#;
        let status: HealthStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_severity_serde_roundtrip() {
        for sev in [Severity::Info, Severity::Warning, Severity::Error] {
            let json = serde_json::to_string(&sev).unwrap();
            let decoded: Severity = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, sev);
        }
    }
}
