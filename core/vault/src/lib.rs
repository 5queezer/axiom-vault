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
pub mod manager;
pub mod operations;
pub mod session;
pub mod tree;

pub use config::{VaultConfig, VaultVersion};
pub use manager::VaultManager;
pub use operations::VaultOperations;
pub use session::{SessionHandle, VaultSession};
pub use tree::{NodeType, TreeNode, VaultTree};
