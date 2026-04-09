//! Vault configuration and metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use subtle::ConstantTimeEq;

use axiomvault_common::{Error, Result, VaultId};
use axiomvault_crypto::recovery::{
    self, create_recovery_verification, generate_master_key, unwrap_key, wrap_key, RecoveryKey,
};
use axiomvault_crypto::{KdfParams, MasterKey, Salt};

/// Vault format version for migration support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VaultVersion {
    pub major: u32,
    pub minor: u32,
}

impl VaultVersion {
    /// Current vault format version.
    /// Bumped to 1.1 to indicate recovery key / key-wrapping support.
    pub const CURRENT: Self = Self { major: 1, minor: 1 };

    /// Check if this version is compatible with the current version.
    pub fn is_compatible(&self) -> bool {
        self.major == Self::CURRENT.major
    }
}

impl std::fmt::Display for VaultVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
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
///
/// ## Key hierarchy (v1.1+)
///
/// A random **master key** is generated at vault creation. It is encrypted
/// ("wrapped") under two independent key-encryption keys (KEKs):
///
/// 1. `wrapped_master_key` -- wrapped with a KEK derived from the user
///    password via Argon2id.
/// 2. `recovery_wrapped_master_key` -- wrapped with a KEK derived from a
///    256-bit recovery key via Blake2b.
///
/// The `key_verification` field lets us cheaply verify the password
/// without unwrapping the master key (AEAD decryption of a known
/// constant).
///
/// ## Legacy format (v1.0)
///
/// In the original format the Argon2id output *was* the master key
/// directly -- there was no wrapping step. Old vaults are detected by
/// the absence of `wrapped_master_key` and can be migrated in-place.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    /// Unique vault identifier.
    pub id: VaultId,
    /// Vault format version.
    pub version: VaultVersion,
    /// Salt for password-based key derivation (Argon2id).
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

    // -- v1.1 fields (key wrapping + recovery) ---------------------------
    /// Master key encrypted (wrapped) with the password-derived KEK.
    /// `None` for legacy v1.0 vaults that have not been migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped_master_key: Option<Vec<u8>>,

    /// Master key encrypted (wrapped) with the recovery-key-derived KEK.
    /// `None` if no recovery key has been set up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_wrapped_master_key: Option<Vec<u8>>,

    /// Verification data for the recovery key (encrypted known constant).
    /// Allows checking whether user-supplied recovery words are correct
    /// without unwrapping the master key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_key_verification: Option<Vec<u8>>,

    /// Recovery key encrypted with the master key so the user can
    /// re-display it later (requires unlocking with password first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_recovery_key: Option<Vec<u8>>,
}

/// Result of creating a new vault configuration.
pub struct VaultConfigCreation {
    /// The vault configuration to persist.
    pub config: VaultConfig,
    /// The master key, passed through to avoid a second Argon2id round on vault creation.
    pub master_key: MasterKey,
    /// The recovery key mnemonic (24 BIP39 words). Show to user once.
    pub recovery_words: String,
}

impl VaultConfig {
    /// Create a new vault configuration with key wrapping and recovery key.
    ///
    /// # Preconditions
    /// - `password` must not be empty
    ///
    /// # Returns
    /// A `VaultConfigCreation` containing the config and the recovery words
    /// that must be shown to the user exactly once.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        id: VaultId,
        password: &[u8],
        provider_type: impl Into<String>,
        provider_config: serde_json::Value,
        kdf_params: KdfParams,
    ) -> Result<VaultConfigCreation> {
        use axiomvault_crypto::{derive_key, encrypt};

        let salt = Salt::generate();

        // 1. Generate a random master key.
        let master_key = generate_master_key();

        // 2. Derive password KEK and wrap the master key.
        let password_kek = derive_key(password, &salt, &kdf_params)?;
        let wrapped_master_key = wrap_key(&master_key, password_kek.as_bytes())?;

        // 3. Create password verification data.
        let verification_plaintext = b"AXIOMVAULT_KEY_VERIFICATION_V1";
        let key_verification = encrypt(password_kek.as_bytes(), verification_plaintext)?;

        // 4. Generate recovery key and wrap the master key with it.
        let recovery_key = RecoveryKey::generate();
        let recovery_kek = recovery_key.derive_kek();
        let recovery_wrapped_master_key = wrap_key(&master_key, &recovery_kek)?;
        let recovery_key_verification = create_recovery_verification(&recovery_key)?;

        // 5. Encrypt the recovery key with the master key so it can be
        //    re-displayed later when the vault is unlocked.
        let encrypted_recovery_key = encrypt(master_key.as_bytes(), recovery_key.as_bytes())?;

        let recovery_words = String::from(&*recovery_key.to_mnemonic()?);

        let now = Utc::now();

        let config = VaultConfig {
            id,
            version: VaultVersion::CURRENT,
            salt,
            kdf_params,
            provider_type: provider_type.into(),
            provider_config,
            created_at: now,
            modified_at: now,
            key_verification,
            wrapped_master_key: Some(wrapped_master_key),
            recovery_wrapped_master_key: Some(recovery_wrapped_master_key),
            recovery_key_verification: Some(recovery_key_verification),
            encrypted_recovery_key: Some(encrypted_recovery_key),
        };

        Ok(VaultConfigCreation {
            config,
            master_key,
            recovery_words,
        })
    }

    /// Check whether this config uses the legacy (v1.0) key model where the
    /// Argon2id output *is* the master key, rather than the wrapped model.
    pub fn is_legacy_format(&self) -> bool {
        self.wrapped_master_key.is_none()
    }

    /// Verify a password against this configuration.
    ///
    /// Returns the **master key** on success so the caller does not need
    /// to derive it a second time (avoids double-KDF cost on unlock).
    ///
    /// For v1.1+ vaults the password-derived KEK unwraps the stored
    /// `wrapped_master_key`. For legacy v1.0 vaults the Argon2id output
    /// is used directly as the master key.
    ///
    /// # Returns
    /// - `Ok(Some(key))` if password is correct
    /// - `Ok(None)` if password is incorrect
    /// - `Err(_)` if verification failed for other reasons
    pub fn verify_password(&self, password: &[u8]) -> Result<Option<MasterKey>> {
        use axiomvault_crypto::{decrypt, derive_key};
        use zeroize::Zeroize;

        let password_kek = derive_key(password, &self.salt, &self.kdf_params)?;

        // First, verify the password by decrypting the verification constant.
        match decrypt(password_kek.as_bytes(), &self.key_verification) {
            Ok(mut plaintext) => {
                let expected = b"AXIOMVAULT_KEY_VERIFICATION_V1";
                let valid = plaintext.len() == expected.len()
                    && bool::from(plaintext.as_slice().ct_eq(expected));
                plaintext.zeroize();
                if !valid {
                    return Ok(None);
                }
            }
            Err(_) => return Ok(None),
        }

        // Password is correct. Now obtain the master key.
        if let Some(ref wrapped) = self.wrapped_master_key {
            // v1.1+: unwrap the master key.
            let master_key = unwrap_key(wrapped, password_kek.as_bytes())?;
            Ok(Some(master_key))
        } else {
            // Legacy v1.0: the KEK *is* the master key.
            Ok(Some(password_kek))
        }
    }

    /// Verify a recovery key and return the master key on success.
    ///
    /// # Returns
    /// - `Ok(Some(key))` if recovery key is correct
    /// - `Ok(None)` if recovery key is incorrect
    /// - `Err(_)` on internal error
    pub fn verify_recovery_key(&self, recovery_key: &RecoveryKey) -> Result<Option<MasterKey>> {
        let verification = self
            .recovery_key_verification
            .as_ref()
            .ok_or_else(|| Error::Vault("No recovery key configured for this vault".to_string()))?;

        if !recovery::verify_recovery_key(recovery_key, verification)? {
            return Ok(None);
        }

        let wrapped = self.recovery_wrapped_master_key.as_ref().ok_or_else(|| {
            Error::Vault("Recovery-wrapped master key missing from config".to_string())
        })?;

        let recovery_kek = recovery_key.derive_kek();
        let master_key = unwrap_key(wrapped, &recovery_kek)?;
        Ok(Some(master_key))
    }

    /// Reset the password using a recovery key.
    ///
    /// This re-wraps the master key with a new password-derived KEK and
    /// updates the password verification data. The recovery key data
    /// remains unchanged.
    pub fn reset_password(
        &mut self,
        recovery_key: &RecoveryKey,
        new_password: &[u8],
    ) -> Result<()> {
        use axiomvault_crypto::{derive_key, encrypt};

        if new_password.is_empty() {
            return Err(Error::InvalidInput(
                "New password cannot be empty".to_string(),
            ));
        }

        // Verify recovery key and get master key.
        let master_key = self
            .verify_recovery_key(recovery_key)?
            .ok_or_else(|| Error::NotPermitted("Invalid recovery key".to_string()))?;

        // Derive new password KEK.
        let new_salt = Salt::generate();
        let new_kek = derive_key(new_password, &new_salt, &self.kdf_params)?;

        // Re-wrap master key.
        let new_wrapped = wrap_key(&master_key, new_kek.as_bytes())?;

        // Re-create password verification.
        let verification_plaintext = b"AXIOMVAULT_KEY_VERIFICATION_V1";
        let new_verification = encrypt(new_kek.as_bytes(), verification_plaintext)?;

        // Update config.
        self.salt = new_salt;
        self.key_verification = new_verification;
        self.wrapped_master_key = Some(new_wrapped);
        self.modified_at = Utc::now();

        Ok(())
    }

    /// Retrieve the stored recovery key by decrypting with the master key.
    ///
    /// Requires the vault to be unlocked (master key available).
    pub fn decrypt_recovery_key(&self, master_key: &MasterKey) -> Result<RecoveryKey> {
        use axiomvault_crypto::decrypt;

        let encrypted = self.encrypted_recovery_key.as_ref().ok_or_else(|| {
            Error::Vault("No encrypted recovery key stored in this vault".to_string())
        })?;

        use zeroize::{Zeroize, Zeroizing};

        let mut plaintext = decrypt(master_key.as_bytes(), encrypted)?;
        if plaintext.len() != 32 {
            return Err(Error::Crypto(format!(
                "Decrypted recovery key has wrong length: expected 32, got {}",
                plaintext.len()
            )));
        }
        let mut bytes = Zeroizing::new([0u8; 32]);
        bytes.copy_from_slice(&plaintext);

        // Best-effort: wipe plaintext buffer containing key material.
        plaintext.zeroize();

        Ok(RecoveryKey::from_bytes(*bytes))
    }

    /// Migrate a legacy v1.0 vault to the v1.1 key-wrapping format.
    ///
    /// For legacy vaults, the Argon2id output was the master key. We keep
    /// that as the master key and simply wrap it under both the password
    /// KEK and a new recovery KEK.
    ///
    /// # Returns
    /// The recovery words to show to the user.
    pub fn migrate_to_v1_1(&mut self, password: &[u8]) -> Result<String> {
        use axiomvault_crypto::encrypt;

        if !self.is_legacy_format() {
            return Err(Error::Vault("Vault is already in v1.1 format".to_string()));
        }

        // In legacy mode, verify_password returns the Argon2id output which
        // serves as BOTH the master key and the password KEK. Reuse it to
        // avoid a second KDF round.
        let master_key = self
            .verify_password(password)?
            .ok_or_else(|| Error::NotPermitted("Invalid password".to_string()))?;

        // In legacy format, KEK == master key, so wrap with itself.
        let wrapped_master_key = wrap_key(&master_key, master_key.as_bytes())?;

        let recovery_key = RecoveryKey::generate();
        let recovery_kek = recovery_key.derive_kek();
        let recovery_wrapped = wrap_key(&master_key, &recovery_kek)?;
        let recovery_verification = create_recovery_verification(&recovery_key)?;
        let encrypted_recovery_key = encrypt(master_key.as_bytes(), recovery_key.as_bytes())?;
        let recovery_words = String::from(&*recovery_key.to_mnemonic()?);

        self.version = VaultVersion::CURRENT;
        self.wrapped_master_key = Some(wrapped_master_key);
        self.recovery_wrapped_master_key = Some(recovery_wrapped);
        self.recovery_key_verification = Some(recovery_verification);
        self.encrypted_recovery_key = Some(encrypted_recovery_key);
        self.modified_at = Utc::now();

        Ok(recovery_words)
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

        // v1.0 should be compatible with v1.1
        let legacy = VaultVersion { major: 1, minor: 0 };
        assert!(legacy.is_compatible());
    }

    #[test]
    fn test_config_creation_and_verification() {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"secure-password";
        let params = KdfParams::moderate();

        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();
        let config = creation.config;

        assert_eq!(creation.recovery_words.split_whitespace().count(), 24);
        assert!(config.wrapped_master_key.is_some());
        assert!(config.recovery_wrapped_master_key.is_some());
        assert!(!config.is_legacy_format());

        assert!(config.verify_password(password).unwrap().is_some());
        assert!(config.verify_password(b"wrong-password").unwrap().is_none());
    }

    #[test]
    fn test_recovery_key_verify_and_reset_password() {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"old-password";
        let params = KdfParams::moderate();

        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();
        let mut config = creation.config;
        let recovery_words = creation.recovery_words;

        let rk = RecoveryKey::from_mnemonic(&recovery_words).unwrap();
        assert!(config.verify_recovery_key(&rk).unwrap().is_some());

        let new_password = b"new-password";
        config.reset_password(&rk, new_password).unwrap();

        assert!(config.verify_password(password).unwrap().is_none());
        assert!(config.verify_password(new_password).unwrap().is_some());
        assert!(config.verify_recovery_key(&rk).unwrap().is_some());
    }

    #[test]
    fn test_decrypt_recovery_key() {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"password";
        let params = KdfParams::moderate();

        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();
        let config = creation.config;
        let recovery_words = creation.recovery_words;

        let master_key = config.verify_password(password).unwrap().unwrap();
        let decrypted_rk = config.decrypt_recovery_key(&master_key).unwrap();
        let decrypted_words = decrypted_rk.to_mnemonic().unwrap();

        assert_eq!(*decrypted_words, recovery_words);
    }

    #[test]
    fn test_master_key_consistency() {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"password";
        let params = KdfParams::moderate();

        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();
        let config = creation.config;
        let recovery_words = creation.recovery_words;

        let mk_from_password = config.verify_password(password).unwrap().unwrap();
        let rk = RecoveryKey::from_mnemonic(&recovery_words).unwrap();
        let mk_from_recovery = config.verify_recovery_key(&rk).unwrap().unwrap();

        assert_eq!(mk_from_password.as_bytes(), mk_from_recovery.as_bytes());
    }

    #[test]
    fn test_config_serialization() {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"test";
        let params = KdfParams::moderate();

        let creation = VaultConfig::new(
            id,
            password,
            "local",
            serde_json::json!({"root": "/tmp/vault"}),
            params,
        )
        .unwrap();
        let config = creation.config;

        let json = config.to_json().unwrap();
        let restored = VaultConfig::from_json(&json).unwrap();

        assert_eq!(restored.id.as_str(), config.id.as_str());
        assert_eq!(restored.provider_type, config.provider_type);
        assert!(restored.wrapped_master_key.is_some());
        assert!(restored.verify_password(password).unwrap().is_some());
    }

    #[test]
    fn test_legacy_format_detection() {
        let id = VaultId::new("legacy").unwrap();
        let password = b"password";
        let params = KdfParams::moderate();
        let salt = Salt::generate();

        let master_key = axiomvault_crypto::derive_key(password, &salt, &params).unwrap();
        let verification_plaintext = b"AXIOMVAULT_KEY_VERIFICATION_V1";
        let key_verification =
            axiomvault_crypto::encrypt(master_key.as_bytes(), verification_plaintext).unwrap();

        let config = VaultConfig {
            id,
            version: VaultVersion { major: 1, minor: 0 },
            salt,
            kdf_params: params,
            provider_type: "memory".to_string(),
            provider_config: serde_json::Value::Null,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            key_verification,
            wrapped_master_key: None,
            recovery_wrapped_master_key: None,
            recovery_key_verification: None,
            encrypted_recovery_key: None,
        };

        assert!(config.is_legacy_format());
        assert!(config.verify_password(password).unwrap().is_some());
        assert!(config.verify_password(b"wrong").unwrap().is_none());
    }

    #[test]
    fn test_migrate_legacy_to_v1_1() {
        let id = VaultId::new("legacy").unwrap();
        let password = b"password";
        let params = KdfParams::moderate();
        let salt = Salt::generate();

        let master_key = axiomvault_crypto::derive_key(password, &salt, &params).unwrap();
        let verification_plaintext = b"AXIOMVAULT_KEY_VERIFICATION_V1";
        let key_verification =
            axiomvault_crypto::encrypt(master_key.as_bytes(), verification_plaintext).unwrap();

        let mut config = VaultConfig {
            id,
            version: VaultVersion { major: 1, minor: 0 },
            salt,
            kdf_params: params,
            provider_type: "memory".to_string(),
            provider_config: serde_json::Value::Null,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            key_verification,
            wrapped_master_key: None,
            recovery_wrapped_master_key: None,
            recovery_key_verification: None,
            encrypted_recovery_key: None,
        };

        let recovery_words = config.migrate_to_v1_1(password).unwrap();
        assert_eq!(recovery_words.split_whitespace().count(), 24);
        assert!(!config.is_legacy_format());

        let mk_after = config.verify_password(password).unwrap().unwrap();
        assert_eq!(master_key.as_bytes(), mk_after.as_bytes());

        let rk = RecoveryKey::from_mnemonic(&recovery_words).unwrap();
        let mk_from_recovery = config.verify_recovery_key(&rk).unwrap().unwrap();
        assert_eq!(master_key.as_bytes(), mk_from_recovery.as_bytes());
    }
}
