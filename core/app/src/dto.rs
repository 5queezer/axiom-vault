//! Data Transfer Objects for the UI boundary.
//!
//! DTOs are plain, serializable structs that cross the FFI/bridge boundary.
//! They carry no behavior and no references to internal domain types.
//!
//! # Debug formatting
//!
//! `zeroize::Zeroizing<T>` *does* implement `Debug` by delegating to inner `T`,
//! so a derived `#[derive(Debug)]` on a struct holding `Zeroizing<String>`
//! would print the password / mnemonic in plaintext. The DTOs in this module
//! that hold secrets therefore implement `Debug` by hand and print
//! `[REDACTED]` for the secret fields, matching `MasterKey` / `FileKey` /
//! `CloudTokens` elsewhere in the codebase.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

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
///
/// `recovery_words` is wrapped in [`Zeroizing`] so the mnemonic is wiped
/// from memory when this DTO is dropped. The field is intentionally not
/// `Serialize`/`Deserialize`-able to discourage callers from logging or
/// persisting the secret. `Clone` is intentionally not derived so the
/// mnemonic cannot accidentally proliferate across the FFI boundary.
///
/// `Debug` is implemented by hand and redacts `recovery_words` — see the
/// module-level note about `Zeroizing<T>` and `Debug` (audit hardening).
pub struct VaultCreatedDto {
    /// Vault info.
    pub info: VaultInfoDto,
    /// BIP39 recovery words (24 words). Must be shown to user exactly once.
    pub recovery_words: Zeroizing<String>,
}

impl std::fmt::Debug for VaultCreatedDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultCreatedDto")
            .field("info", &self.info)
            .field("recovery_words", &"[REDACTED]")
            .finish()
    }
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
///
/// `password` is held in [`Zeroizing`] so the secret is wiped from memory
/// when the params are dropped. The struct intentionally does not implement
/// `Serialize`/`Deserialize` because callers should never persist or log it.
///
/// `Debug` is implemented by hand and redacts `password` — see the
/// module-level note about `Zeroizing<T>` and `Debug`.
pub struct CreateVaultParams {
    /// Vault identifier.
    pub vault_id: String,
    /// Password for the vault.
    pub password: Zeroizing<String>,
    /// Storage provider type.
    pub provider_type: String,
    /// Provider-specific configuration.
    pub provider_config: serde_json::Value,
}

impl std::fmt::Debug for CreateVaultParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateVaultParams")
            .field("vault_id", &self.vault_id)
            .field("password", &"[REDACTED]")
            .field("provider_type", &self.provider_type)
            .field("provider_config", &self.provider_config)
            .finish()
    }
}

/// Parameters for opening an existing vault.
///
/// `password` is held in [`Zeroizing`] so the secret is wiped from memory
/// when the params are dropped.
///
/// `Debug` is implemented by hand and redacts `password` — see the
/// module-level note about `Zeroizing<T>` and `Debug`.
pub struct OpenVaultParams {
    /// Password for the vault.
    pub password: Zeroizing<String>,
    /// Storage provider type.
    pub provider_type: String,
    /// Provider-specific configuration.
    pub provider_config: serde_json::Value,
}

impl std::fmt::Debug for OpenVaultParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenVaultParams")
            .field("password", &"[REDACTED]")
            .field("provider_type", &self.provider_type)
            .field("provider_config", &self.provider_config)
            .finish()
    }
}

/// Parameters for recovering a vault with recovery words.
///
/// Both `recovery_words` and `new_password` are held in [`Zeroizing`] so the
/// secrets are wiped from memory when the params are dropped.
///
/// `Debug` is implemented by hand and redacts both secret fields — see the
/// module-level note about `Zeroizing<T>` and `Debug`.
pub struct RecoverVaultParams {
    /// BIP39 recovery words.
    pub recovery_words: Zeroizing<String>,
    /// New password.
    pub new_password: Zeroizing<String>,
    /// Storage provider type.
    pub provider_type: String,
    /// Provider-specific configuration.
    pub provider_config: serde_json::Value,
}

impl std::fmt::Debug for RecoverVaultParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoverVaultParams")
            .field("recovery_words", &"[REDACTED]")
            .field("new_password", &"[REDACTED]")
            .field("provider_type", &self.provider_type)
            .field("provider_config", &self.provider_config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit hardening: `Zeroizing<T>` delegates `Debug` to inner `T`, so a
    /// derived `Debug` on these DTOs would print the password / mnemonic in
    /// plaintext. The hand-rolled `Debug` impls must redact secrets.
    #[test]
    fn create_vault_params_debug_redacts_password() {
        let params = CreateVaultParams {
            vault_id: "vault-1".to_string(),
            password: Zeroizing::new("super-secret-password-do-not-print".to_string()),
            provider_type: "local".to_string(),
            provider_config: serde_json::json!({}),
        };
        let s = format!("{:?}", params);
        assert!(
            !s.contains("super-secret-password-do-not-print"),
            "CreateVaultParams Debug leaked password: {}",
            s
        );
        assert!(
            s.contains("[REDACTED]"),
            "CreateVaultParams Debug should mark redacted fields: {}",
            s
        );
        // Non-secret fields should still be visible.
        assert!(s.contains("vault-1"));
        assert!(s.contains("local"));
    }

    #[test]
    fn open_vault_params_debug_redacts_password() {
        let params = OpenVaultParams {
            password: Zeroizing::new("another-secret-passphrase".to_string()),
            provider_type: "gdrive".to_string(),
            provider_config: serde_json::json!({"folder": "vault"}),
        };
        let s = format!("{:?}", params);
        assert!(
            !s.contains("another-secret-passphrase"),
            "OpenVaultParams Debug leaked password: {}",
            s
        );
        assert!(s.contains("[REDACTED]"));
        assert!(s.contains("gdrive"));
    }

    #[test]
    fn recover_vault_params_debug_redacts_both_secrets() {
        let params = RecoverVaultParams {
            recovery_words: Zeroizing::new(
                "abandon ability able about above absent absorb abstract absurd abuse access \
                 accident account accuse achieve acid acoustic acquire across act action actor \
                 actress actual"
                    .to_string(),
            ),
            new_password: Zeroizing::new("brand-new-password-456".to_string()),
            provider_type: "local".to_string(),
            provider_config: serde_json::json!({}),
        };
        let s = format!("{:?}", params);
        assert!(
            !s.contains("abandon ability"),
            "RecoverVaultParams Debug leaked recovery_words: {}",
            s
        );
        assert!(
            !s.contains("brand-new-password-456"),
            "RecoverVaultParams Debug leaked new_password: {}",
            s
        );
        // Both fields should appear as [REDACTED].
        let redacted_count = s.matches("[REDACTED]").count();
        assert!(
            redacted_count >= 2,
            "RecoverVaultParams Debug should redact both secret fields, got `{}`",
            s
        );
    }

    #[test]
    fn vault_created_dto_debug_redacts_recovery_words() {
        let dto = VaultCreatedDto {
            info: VaultInfoDto {
                id: "vault-1".to_string(),
                provider_type: "local".to_string(),
                is_unlocked: true,
            },
            recovery_words: Zeroizing::new(
                "abandon ability able about above absent absorb abstract absurd abuse access \
                 accident"
                    .to_string(),
            ),
        };
        let s = format!("{:?}", dto);
        assert!(
            !s.contains("abandon ability"),
            "VaultCreatedDto Debug leaked recovery_words: {}",
            s
        );
        assert!(s.contains("[REDACTED]"));
        // Non-secret info should still be visible.
        assert!(s.contains("vault-1"));
    }
}
