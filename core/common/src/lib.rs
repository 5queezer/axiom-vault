//! Common utilities and types shared across AxiomVault modules.
//!
//! This module provides foundational types that are used throughout the codebase,
//! ensuring consistency and type safety.

pub mod error;
pub mod health;
pub mod secure_file;
pub mod types;

pub use error::{Error, Result};
pub use health::{DiagnosticResult, HealthReport, HealthStatus, Severity};
pub use secure_file::{write_sensitive_file, SensitiveFileMode};
pub use types::{VaultId, VaultPath};
