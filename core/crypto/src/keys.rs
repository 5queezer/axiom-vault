//! Key types with secure memory handling.
//!
//! All key types automatically zeroize their memory on drop to prevent
//! sensitive data from persisting in memory.

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};
use std::fmt;

/// Length of encryption keys in bytes (256-bit).
pub const KEY_LENGTH: usize = 32;

/// Master key derived from user password.
///
/// This key is the root of the key hierarchy and is used to derive
/// other keys for file and directory encryption.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey {
    key: [u8; KEY_LENGTH],
}

impl MasterKey {
    /// Create a master key from raw bytes.
    ///
    /// # Preconditions
    /// - `key` must be exactly KEY_LENGTH bytes
    ///
    /// # Postconditions
    /// - Returns a MasterKey that will zeroize on drop
    ///
    /// # Errors
    /// - Returns error if key length is incorrect
    pub fn from_bytes(key: [u8; KEY_LENGTH]) -> Self {
        Self { key }
    }

    /// Get the key bytes.
    ///
    /// # Security
    /// The returned slice should be used immediately and not stored.
    pub fn as_bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.key
    }

    /// Derive a file key from this master key and a file-specific identifier.
    ///
    /// Uses blake2b for secure key derivation.
    pub fn derive_file_key(&self, file_id: &[u8]) -> FileKey {
        use blake2::{Blake2b, Digest};
        use blake2::digest::consts::U32;

        let mut hasher = Blake2b::<U32>::new();
        hasher.update(&self.key);
        hasher.update(file_id);
        hasher.update(b"filekey");

        let result = hasher.finalize();
        let mut derived = [0u8; KEY_LENGTH];
        derived.copy_from_slice(&result);
        FileKey::from_bytes(derived)
    }

    /// Derive a directory key from this master key.
    pub fn derive_directory_key(&self, dir_id: &[u8]) -> DirectoryKey {
        use blake2::{Blake2b, Digest};
        use blake2::digest::consts::U32;

        let mut hasher = Blake2b::<U32>::new();
        hasher.update(&self.key);
        hasher.update(dir_id);
        hasher.update(b"dirkey");

        let result = hasher.finalize();
        let mut derived = [0u8; KEY_LENGTH];
        derived.copy_from_slice(&result);
        DirectoryKey::from_bytes(derived)
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MasterKey([REDACTED])")
    }
}

/// Key for encrypting file contents.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct FileKey {
    key: [u8; KEY_LENGTH],
}

impl FileKey {
    /// Create a file key from raw bytes.
    pub fn from_bytes(key: [u8; KEY_LENGTH]) -> Self {
        Self { key }
    }

    /// Get the key bytes.
    pub fn as_bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.key
    }

    /// Generate a random file key.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = [0u8; KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut key);
        Self { key }
    }
}

impl fmt::Debug for FileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileKey([REDACTED])")
    }
}

/// Key for encrypting directory structure and filenames.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct DirectoryKey {
    key: [u8; KEY_LENGTH],
}

impl DirectoryKey {
    /// Create a directory key from raw bytes.
    pub fn from_bytes(key: [u8; KEY_LENGTH]) -> Self {
        Self { key }
    }

    /// Get the key bytes.
    pub fn as_bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.key
    }

    /// Generate a random directory key.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = [0u8; KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut key);
        Self { key }
    }
}

impl fmt::Debug for DirectoryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DirectoryKey([REDACTED])")
    }
}

/// Salt for key derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Salt(pub [u8; 32]);

impl Salt {
    /// Generate a random salt.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut salt = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut salt);
        Self(salt)
    }

    /// Create from bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get the salt bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_master_key_derive_file_key() {
        let master = MasterKey::from_bytes([1u8; KEY_LENGTH]);
        let file_id = b"test-file";

        let key1 = master.derive_file_key(file_id);
        let key2 = master.derive_file_key(file_id);

        // Same input should produce same key
        assert_eq!(key1.as_bytes(), key2.as_bytes());

        // Different input should produce different key
        let key3 = master.derive_file_key(b"other-file");
        assert_ne!(key1.as_bytes(), key3.as_bytes());
    }

    #[test]
    fn test_file_key_generate() {
        let key1 = FileKey::generate();
        let key2 = FileKey::generate();

        // Random keys should be different
        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_salt_generate() {
        let salt1 = Salt::generate();
        let salt2 = Salt::generate();

        // Random salts should be different
        assert_ne!(salt1.as_bytes(), salt2.as_bytes());
    }
}
