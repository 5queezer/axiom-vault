//! Dropbox storage provider for AxiomVault.
//!
//! This module provides a storage backend using Dropbox with:
//! - OAuth2 authentication with automatic token refresh
//! - Chunked upload sessions for large files
//! - Path-based operations (no ID resolution needed)
//! - Full StorageProvider trait implementation

pub mod auth;
pub mod client;
pub mod provider;

pub use auth::{DropboxAuthConfig, DropboxAuthManager, DropboxTokenManager, DropboxTokens};
pub use client::DropboxClient;
pub use provider::{create_dropbox_provider, DropboxConfig, DropboxProvider};
