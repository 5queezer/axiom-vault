//! Vault engine for AxiomVault.
//!
//! This module provides:
//! - Vault creation and lifecycle management
//! - Encrypted file and directory operations
//! - Metadata management and persistence
//! - Session handling with secure key management
//!
//! # Architecture
//! The vault module sits between the user interface and storage providers,
//! handling all encryption/decryption operations transparently.

pub mod config;
pub mod health;
pub mod manager;
pub mod migration;
pub mod operations;
pub mod session;
pub mod tree;

pub use config::{VaultConfig, VaultVersion};
// Re-export unified health types from common alongside vault-specific check functions.
pub use axiomvault_common::health::{DiagnosticResult, HealthReport, HealthStatus, Severity};
pub use health::{check_vault_health, check_vault_structure};
pub use manager::{VaultCreation, VaultManager};
pub use migration::{check_migration_needed, Migration, MigrationRegistry, MigrationStatus};
pub use operations::VaultOperations;
pub use session::{SessionHandle, VaultSession};
pub use tree::{NodeType, TreeNode, VaultTree};
