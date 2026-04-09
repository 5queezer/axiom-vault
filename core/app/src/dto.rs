//! Data Transfer Objects for the UI boundary.
//!
//! DTOs are plain, serializable structs that cross the FFI/bridge boundary.
//! They carry no behavior and no references to internal domain types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Information about an open vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultInfoDto {
    /// Vault identifier.
    pub id: String,
    /// Storage provider type (e.g. "local", "gdrive").
    pub provider_type: String,
    /// Whether the vault is currently unlocked.
    pub is_unlocked: bool,
}

/// Result of vault creation, including the recovery words.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultCreatedDto {
    /// Vault info.
    pub info: VaultInfoDto,
    /// BIP39 recovery words (24 words). Must be shown to user exactly once.
    pub recovery_words: String,
}

/// A single entry in a directory listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryEntryDto {
    /// Display name.
    pub name: String,
    /// Full vault path.
    pub path: String,
    /// Whether this entry is a directory.
    pub is_directory: bool,
    /// File size in bytes (None for directories).
    pub size: Option<u64>,
    /// Last modified timestamp.
    pub modified_at: Option<DateTime<Utc>>,
}

/// File metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadataDto {
    /// Display name.
    pub name: String,
    /// Full vault path.
    pub path: String,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// File size in bytes.
    pub size: Option<u64>,
}

/// Parameters for creating a new vault.
#[derive(Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct CreateVaultParams {
    /// Vault identifier.
    #[zeroize(skip)]
    pub vault_id: String,
    /// Password for the vault.
    pub password: String,
    /// Storage provider type.
    #[zeroize(skip)]
    pub provider_type: String,
    /// Provider-specific configuration.
    #[zeroize(skip)]
    pub provider_config: serde_json::Value,
}

/// Parameters for opening an existing vault.
#[derive(Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct OpenVaultParams {
    /// Password for the vault.
    pub password: String,
    /// Storage provider type.
    #[zeroize(skip)]
    pub provider_type: String,
    /// Provider-specific configuration.
    #[zeroize(skip)]
    pub provider_config: serde_json::Value,
}

/// Parameters for recovering a vault with recovery words.
#[derive(Debug, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct RecoverVaultParams {
    /// BIP39 recovery words.
    pub recovery_words: String,
    /// New password.
    pub new_password: String,
    /// Storage provider type.
    #[zeroize(skip)]
    pub provider_type: String,
    /// Provider-specific configuration.
    #[zeroize(skip)]
    pub provider_config: serde_json::Value,
}
