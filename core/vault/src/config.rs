//! Vault configuration and metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use axiomvault_common::{Error, Result, VaultId};
use axiomvault_crypto::{KdfParams, Salt};

/// Vault format version for migration support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultVersion {
    pub major: u32,
    pub minor: u32,
}

impl VaultVersion {
    /// Current vault format version.
    pub const CURRENT: Self = Self { major: 1, minor: 0 };

    /// Check if this version is compatible with the current version.
    pub fn is_compatible(&self) -> bool {
        self.major == Self::CURRENT.major
    }
}

impl Default for VaultVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

/// Encrypted vault configuration.
///
/// This structure is stored at the vault root and contains all
/// necessary metadata for vault operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    /// Unique vault identifier.
    pub id: VaultId,
    /// Vault format version.
    pub version: VaultVersion,
    /// Salt for master key derivation.
    pub salt: Salt,
    /// KDF parameters.
    pub kdf_params: KdfParams,
    /// Storage provider type (e.g., "local", "gdrive").
    pub provider_type: String,
    /// Provider-specific configuration.
    pub provider_config: serde_json::Value,
    /// Vault creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp.
    pub modified_at: DateTime<Utc>,
    /// Encrypted master key verification data.
    /// This is used to verify the password without storing the key.
    pub key_verification: Vec<u8>,
}

impl VaultConfig {
    /// Create a new vault configuration.
    ///
    /// # Preconditions
    /// - `id` must be valid
    /// - `password` must not be empty
    /// - `provider_type` must be a valid provider name
    ///
    /// # Postconditions
    /// - Returns configuration with derived key verification
    /// - Salt is randomly generated
    ///
    /// # Errors
    /// - Invalid vault ID
    /// - Password empty
    /// - KDF failure
    pub fn new(
        id: VaultId,
        password: &[u8],
        provider_type: impl Into<String>,
        provider_config: serde_json::Value,
        kdf_params: KdfParams,
    ) -> Result<Self> {
        use axiomvault_crypto::{derive_key, encrypt};

        let salt = Salt::generate();
        let master_key = derive_key(password, &salt, &kdf_params)?;

        // Create verification data: encrypt a known constant
        let verification_plaintext = b"AXIOMVAULT_KEY_VERIFICATION_V1";
        let key_verification = encrypt(master_key.as_bytes(), verification_plaintext)?;

        let now = Utc::now();

        Ok(Self {
            id,
            version: VaultVersion::CURRENT,
            salt,
            kdf_params,
            provider_type: provider_type.into(),
            provider_config,
            created_at: now,
            modified_at: now,
            key_verification,
        })
    }

    /// Verify a password against this configuration.
    ///
    /// # Returns
    /// - `Ok(true)` if password is correct
    /// - `Ok(false)` if password is incorrect
    /// - `Err(_)` if verification failed for other reasons
    pub fn verify_password(&self, password: &[u8]) -> Result<bool> {
        use axiomvault_crypto::{decrypt, derive_key};

        let master_key = derive_key(password, &self.salt, &self.kdf_params)?;

        match decrypt(master_key.as_bytes(), &self.key_verification) {
            Ok(plaintext) => {
                let expected = b"AXIOMVAULT_KEY_VERIFICATION_V1";
                Ok(plaintext == expected)
            }
            Err(_) => Ok(false), // Decryption failed = wrong password
        }
    }

    /// Serialize configuration to JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Deserialize configuration from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Serialize to bytes for storage.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| Error::Serialization(e.to_string()))
    }
}

/// Configuration file name in vault root.
pub const CONFIG_FILENAME: &str = "vault.config";

/// Data directory name in vault root.
pub const DATA_DIRNAME: &str = "d";

/// Metadata directory name in vault root.
pub const META_DIRNAME: &str = "m";

/// Tree state filename in metadata directory.
pub const TREE_FILENAME: &str = "tree.json";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_version_compatibility() {
        let current = VaultVersion::CURRENT;
        assert!(current.is_compatible());

        let incompatible = VaultVersion { major: 2, minor: 0 };
        assert!(!incompatible.is_compatible());
    }

    #[test]
    fn test_config_creation_and_verification() {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"secure-password";
        let params = KdfParams::moderate();

        let config =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();

        assert!(config.verify_password(password).unwrap());
        assert!(!config.verify_password(b"wrong-password").unwrap());
    }

    #[test]
    fn test_config_serialization() {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"test";
        let params = KdfParams::moderate();

        let config = VaultConfig::new(
            id,
            password,
            "local",
            serde_json::json!({"root": "/tmp/vault"}),
            params,
        )
        .unwrap();

        let json = config.to_json().unwrap();
        let restored = VaultConfig::from_json(&json).unwrap();

        assert_eq!(restored.id.as_str(), config.id.as_str());
        assert_eq!(restored.provider_type, config.provider_type);
    }
}
