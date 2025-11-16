//! Key derivation using Argon2id.
//!
//! Argon2id is a memory-hard password hashing function that provides
//! resistance to both GPU and time-memory trade-off attacks.

use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};

use crate::keys::{MasterKey, Salt, KEY_LENGTH};
use axiomvault_common::{Error, Result};

/// Parameters for Argon2id key derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory cost in KiB (e.g., 65536 = 64 MiB).
    pub memory_cost: u32,
    /// Number of iterations.
    pub time_cost: u32,
    /// Degree of parallelism.
    pub parallelism: u32,
}

impl KdfParams {
    /// Create parameters suitable for interactive use.
    ///
    /// These parameters provide a balance between security and usability,
    /// targeting approximately 0.5-1 second of derivation time.
    pub fn interactive() -> Self {
        Self {
            memory_cost: 65536, // 64 MiB
            time_cost: 3,
            parallelism: 4,
        }
    }

    /// Create parameters suitable for sensitive data.
    ///
    /// Higher security parameters that may take several seconds.
    pub fn sensitive() -> Self {
        Self {
            memory_cost: 262144, // 256 MiB
            time_cost: 4,
            parallelism: 4,
        }
    }

    /// Create moderate parameters for mobile devices.
    pub fn moderate() -> Self {
        Self {
            memory_cost: 32768, // 32 MiB
            time_cost: 3,
            parallelism: 2,
        }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::interactive()
    }
}

/// Derive a master key from a password and salt using Argon2id.
///
/// # Preconditions
/// - `password` must not be empty
/// - `params` must have valid Argon2id parameters
///
/// # Postconditions
/// - Returns a MasterKey derived from the password
/// - The derived key is deterministic given the same inputs
///
/// # Errors
/// - Returns error if password is empty
/// - Returns error if Argon2id parameters are invalid
///
/// # Security
/// - Password is not stored or logged
/// - Memory is zeroized after derivation
pub fn derive_key(password: &[u8], salt: &Salt, params: &KdfParams) -> Result<MasterKey> {
    if password.is_empty() {
        return Err(Error::InvalidInput("Password cannot be empty".to_string()));
    }

    let argon2_params = Params::new(
        params.memory_cost,
        params.time_cost,
        params.parallelism,
        Some(KEY_LENGTH),
    )
    .map_err(|e| Error::Crypto(format!("Invalid KDF parameters: {}", e)))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params);

    let mut key_bytes = [0u8; KEY_LENGTH];
    argon2
        .hash_password_into(password, salt.as_bytes(), &mut key_bytes)
        .map_err(|e| Error::Crypto(format!("Key derivation failed: {}", e)))?;

    Ok(MasterKey::from_bytes(key_bytes))
}

/// Verify that a password produces the expected key.
///
/// This performs constant-time comparison to prevent timing attacks.
pub fn verify_password(
    password: &[u8],
    salt: &Salt,
    params: &KdfParams,
    expected: &MasterKey,
) -> Result<bool> {
    let derived = derive_key(password, salt, params)?;

    // Constant-time comparison
    let mut equal = true;
    for (a, b) in derived.as_bytes().iter().zip(expected.as_bytes().iter()) {
        equal &= a == b;
    }

    Ok(equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_key_deterministic() {
        let password = b"test-password-123";
        let salt = Salt::from_bytes([42u8; 32]);
        let params = KdfParams::moderate();

        let key1 = derive_key(password, &salt, &params).unwrap();
        let key2 = derive_key(password, &salt, &params).unwrap();

        assert_eq!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_derive_key_different_salt() {
        let password = b"test-password-123";
        let salt1 = Salt::from_bytes([1u8; 32]);
        let salt2 = Salt::from_bytes([2u8; 32]);
        let params = KdfParams::moderate();

        let key1 = derive_key(password, &salt1, &params).unwrap();
        let key2 = derive_key(password, &salt2, &params).unwrap();

        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_derive_key_different_password() {
        let salt = Salt::from_bytes([42u8; 32]);
        let params = KdfParams::moderate();

        let key1 = derive_key(b"password1", &salt, &params).unwrap();
        let key2 = derive_key(b"password2", &salt, &params).unwrap();

        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_derive_key_empty_password_fails() {
        let salt = Salt::generate();
        let params = KdfParams::moderate();

        assert!(derive_key(b"", &salt, &params).is_err());
    }

    #[test]
    fn test_verify_password() {
        let password = b"secure-password";
        let salt = Salt::from_bytes([99u8; 32]);
        let params = KdfParams::moderate();

        let key = derive_key(password, &salt, &params).unwrap();
        assert!(verify_password(password, &salt, &params, &key).unwrap());
        assert!(!verify_password(b"wrong-password", &salt, &params, &key).unwrap());
    }
}
