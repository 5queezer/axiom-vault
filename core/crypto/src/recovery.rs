//! Recovery key generation and key wrapping.
//!
//! Provides:
//! - Random master key generation
//! - Key wrapping (encrypting a master key with a KEK)
//! - Recovery key generation as BIP39 mnemonic words
//! - Deriving a KEK from a recovery key via Blake2b (high-entropy input,
//!   no need for slow Argon2id)
//!
//! # Design
//! The vault's actual master key is randomly generated and then "wrapped"
//! (encrypted with XChaCha20-Poly1305) under two separate key-encryption
//! keys (KEKs):
//!
//! 1. **Password KEK** -- derived from the user password via Argon2id.
//! 2. **Recovery KEK** -- derived from a 256-bit recovery key via Blake2b.
//!
//! Both wrapped copies are stored in `VaultConfig`. The recovery key
//! itself is shown to the user once (as 24 BIP39 words) and never stored
//! in plaintext.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::aead;
use crate::keys::{MasterKey, KEY_LENGTH};
use axiomvault_common::{Error, Result};

/// Context string for deriving a recovery KEK via Blake2b.
const RECOVERY_KEK_CONTEXT: &[u8] = b"axiomvault_recovery_kek_v1";

/// A 256-bit recovery key that can be encoded as BIP39 mnemonic words.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey {
    entropy: [u8; KEY_LENGTH],
}

impl RecoveryKey {
    /// Generate a new random recovery key.
    pub fn generate() -> Self {
        use rand::RngExt;
        let mut entropy = [0u8; KEY_LENGTH];
        rand::rng().fill(&mut entropy[..]);
        Self { entropy }
    }

    /// Create from raw entropy bytes.
    pub fn from_bytes(entropy: [u8; KEY_LENGTH]) -> Self {
        Self { entropy }
    }

    /// Get raw entropy bytes.
    pub fn as_bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.entropy
    }

    /// Encode recovery key as BIP39 mnemonic words (24 words for 256 bits).
    pub fn to_mnemonic(&self) -> Result<String> {
        let mnemonic = bip39::Mnemonic::from_entropy(&self.entropy)
            .map_err(|e| Error::Crypto(format!("Failed to encode recovery key: {}", e)))?;
        Ok(mnemonic.to_string())
    }

    /// Decode recovery key from BIP39 mnemonic words.
    pub fn from_mnemonic(words: &str) -> Result<Self> {
        let mnemonic: bip39::Mnemonic = words
            .parse()
            .map_err(|e| Error::Crypto(format!("Invalid recovery key words: {}", e)))?;
        let entropy = mnemonic.to_entropy();
        if entropy.len() != KEY_LENGTH {
            return Err(Error::Crypto(format!(
                "Invalid recovery key length: expected {} bytes, got {}",
                KEY_LENGTH,
                entropy.len()
            )));
        }
        let mut bytes = [0u8; KEY_LENGTH];
        bytes.copy_from_slice(&entropy);
        Ok(Self { entropy: bytes })
    }

    /// Derive a key-encryption key (KEK) from this recovery key using Blake2b.
    ///
    /// Since the recovery key is already 256 bits of high-entropy randomness,
    /// a fast hash (Blake2b) with domain separation is sufficient -- no need
    /// for the slow Argon2id used for passwords.
    pub fn derive_kek(&self) -> [u8; KEY_LENGTH] {
        let mut hasher = Blake2b::<U32>::new();
        hasher.update(self.entropy);
        hasher.update(RECOVERY_KEK_CONTEXT);
        let result = hasher.finalize();
        let mut kek = [0u8; KEY_LENGTH];
        kek.copy_from_slice(&result);
        kek
    }
}

impl std::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RecoveryKey([REDACTED])")
    }
}

// -- Key wrapping helpers ------------------------------------------------

/// Generate a new random master key.
pub fn generate_master_key() -> MasterKey {
    use rand::RngExt;
    let mut key = [0u8; KEY_LENGTH];
    rand::rng().fill(&mut key[..]);
    MasterKey::from_bytes(key)
}

/// Wrap (encrypt) a master key with a key-encryption key.
///
/// Returns nonce || ciphertext || tag (same layout as `aead::encrypt`).
pub fn wrap_key(master_key: &MasterKey, kek: &[u8; KEY_LENGTH]) -> Result<Vec<u8>> {
    aead::encrypt(kek, master_key.as_bytes())
}

/// Unwrap (decrypt) a master key with a key-encryption key.
pub fn unwrap_key(wrapped: &[u8], kek: &[u8; KEY_LENGTH]) -> Result<MasterKey> {
    let mut plaintext = aead::decrypt(kek, wrapped)?;
    if plaintext.len() != KEY_LENGTH {
        return Err(Error::Crypto(format!(
            "Unwrapped key has wrong length: expected {}, got {}",
            KEY_LENGTH,
            plaintext.len()
        )));
    }

    let mut key = Zeroizing::new([0u8; KEY_LENGTH]);
    key.copy_from_slice(&plaintext);

    // Best-effort: wipe plaintext buffer containing key material.
    plaintext.zeroize();

    Ok(MasterKey::from_bytes(*key))
}

/// Verification constant used to validate recovery keys.
pub const RECOVERY_VERIFICATION_PLAINTEXT: &[u8] = b"AXIOMVAULT_RECOVERY_VERIFICATION_V1";

/// Create verification data for a recovery key.
///
/// Encrypts a known constant with the recovery-derived KEK so we can
/// check if the user's recovery words are correct without storing them.
pub fn create_recovery_verification(recovery_key: &RecoveryKey) -> Result<Vec<u8>> {
    let kek = recovery_key.derive_kek();
    aead::encrypt(&kek, RECOVERY_VERIFICATION_PLAINTEXT)
}

/// Verify recovery key against stored verification data.
///
/// Returns `true` if the recovery key is correct.
pub fn verify_recovery_key(recovery_key: &RecoveryKey, verification: &[u8]) -> Result<bool> {
    use subtle::ConstantTimeEq;
    let kek = recovery_key.derive_kek();
    match aead::decrypt(&kek, verification) {
        Ok(plaintext) => Ok(plaintext.len() == RECOVERY_VERIFICATION_PLAINTEXT.len()
            && bool::from(plaintext.as_slice().ct_eq(RECOVERY_VERIFICATION_PLAINTEXT))),
        Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recovery_key_mnemonic_roundtrip() {
        let key = RecoveryKey::generate();
        let words = key.to_mnemonic().unwrap();

        // BIP39 256-bit = 24 words
        assert_eq!(words.split_whitespace().count(), 24);

        let restored = RecoveryKey::from_mnemonic(&words).unwrap();
        assert_eq!(key.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn test_recovery_key_invalid_mnemonic() {
        let result = RecoveryKey::from_mnemonic("not valid words at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_master_key_is_random() {
        let k1 = generate_master_key();
        let k2 = generate_master_key();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_wrap_unwrap_roundtrip() {
        let master = generate_master_key();
        let kek = [42u8; KEY_LENGTH];

        let wrapped = wrap_key(&master, &kek).unwrap();
        let unwrapped = unwrap_key(&wrapped, &kek).unwrap();

        assert_eq!(master.as_bytes(), unwrapped.as_bytes());
    }

    #[test]
    fn test_unwrap_wrong_kek_fails() {
        let master = generate_master_key();
        let kek1 = [1u8; KEY_LENGTH];
        let kek2 = [2u8; KEY_LENGTH];

        let wrapped = wrap_key(&master, &kek1).unwrap();
        assert!(unwrap_key(&wrapped, &kek2).is_err());
    }

    #[test]
    fn test_recovery_verification() {
        let rk = RecoveryKey::generate();
        let verification = create_recovery_verification(&rk).unwrap();

        assert!(verify_recovery_key(&rk, &verification).unwrap());

        let rk2 = RecoveryKey::generate();
        assert!(!verify_recovery_key(&rk2, &verification).unwrap());
    }

    #[test]
    fn test_recovery_kek_deterministic() {
        let key = RecoveryKey::from_bytes([99u8; KEY_LENGTH]);
        let kek1 = key.derive_kek();
        let kek2 = key.derive_kek();
        assert_eq!(kek1, kek2);
    }

    #[test]
    fn test_recovery_kek_different_keys() {
        let k1 = RecoveryKey::from_bytes([1u8; KEY_LENGTH]);
        let k2 = RecoveryKey::from_bytes([2u8; KEY_LENGTH]);
        assert_ne!(k1.derive_kek(), k2.derive_kek());
    }
}
