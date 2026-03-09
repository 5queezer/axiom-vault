//! OneDrive storage provider for AxiomVault.
//!
//! This module provides a storage backend using Microsoft OneDrive with:
//! - OAuth2 authentication via Azure AD with automatic token refresh
//! - Resumable uploads for large files
//! - Path-based addressing via Microsoft Graph API
//! - Full StorageProvider trait implementation

pub mod auth;
pub mod client;
pub mod provider;

pub use auth::{OneDriveAuthConfig, OneDriveAuthManager, OneDriveTokenManager, OneDriveTokens};
pub use client::{DriveItem, OneDriveClient};
pub use provider::{create_onedrive_provider, OneDriveConfig, OneDriveProvider};
