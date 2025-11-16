//! Storage provider abstraction for AxiomVault.
//!
//! This module provides a trait-based interface for different storage backends
//! (Google Drive, local filesystem, iCloud, etc.) and a provider registry
//! for dynamic provider resolution.
//!
//! # Design Principles
//! - Provider isolation: No provider-specific logic in vault or crypto modules
//! - Async operations: All I/O operations are async
//! - Streaming support: Large files are handled via streams
//! - Unified error semantics: Consistent error types across providers

pub mod gdrive;
pub mod local;
pub mod memory;
pub mod provider;
pub mod registry;

pub use gdrive::{GDriveConfig, GDriveProvider};
pub use local::LocalProvider;
pub use memory::MemoryProvider;
pub use provider::{ConflictResolution, Metadata, StorageProvider};
pub use registry::{create_default_registry, ProviderFactory, ProviderRegistry};
