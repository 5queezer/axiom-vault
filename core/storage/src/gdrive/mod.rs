//! Google Drive storage provider for AxiomVault.
//!
//! This module provides a storage backend using Google Drive with:
//! - OAuth2 authentication with automatic token refresh
//! - Chunked/resumable uploads for large files
//! - Path-to-ID caching for performance
//! - Full StorageProvider trait implementation

pub mod auth;
pub mod client;
pub mod provider;

pub use auth::{AuthConfig, AuthManager, TokenManager, Tokens};
pub use client::DriveClient;
pub use provider::{create_gdrive_provider, GDriveConfig, GDriveProvider};
