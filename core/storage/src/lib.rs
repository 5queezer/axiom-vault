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

pub mod composite;
pub mod dropbox;
pub mod gdrive;
pub mod health;
pub mod icloud;
pub mod local;
pub mod memory;
pub mod onedrive;
pub mod provider;
pub mod registry;
pub mod shard_map;

pub use composite::{CompositeConfig, CompositeStorageProvider, RaidMode};
pub use health::{HealthConfig, HealthStatus, ProviderHealth};
pub use dropbox::{DropboxConfig, DropboxProvider};
pub use gdrive::{GDriveConfig, GDriveProvider};
pub use icloud::{ICloudConfig, ICloudProvider};
pub use local::LocalProvider;
pub use memory::MemoryProvider;
pub use onedrive::{OneDriveConfig, OneDriveProvider};
pub use provider::{ConflictResolution, Metadata, StorageProvider};
pub use registry::{create_default_registry, ProviderFactory, ProviderRegistry};
pub use shard_map::{ChunkEntry, ErasureParams, ShardLocation, ShardMap};
