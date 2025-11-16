//! Common utilities and types shared across AxiomVault modules.
//!
//! This module provides foundational types that are used throughout the codebase,
//! ensuring consistency and type safety.

pub mod error;
pub mod types;

pub use error::{Error, Result};
pub use types::{VaultId, VaultPath};
