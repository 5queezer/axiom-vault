//! Authenticated encryption using XChaCha20-Poly1305.
//!
//! XChaCha20-Poly1305 provides both confidentiality and authenticity,
//! with a 24-byte nonce that is safe for random generation.

use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, generic_array::GenericArray},
    XChaCha20Poly1305,
};

use axiomvault_common::{Error, Result};
use crate::keys::KEY_LENGTH;

/// Nonce size for XChaCha20-Poly1305 (24 bytes).
pub const NONCE_SIZE: usize = 24;

/// Authentication tag size (16 bytes).
pub const TAG_SIZE: usize = 16;

/// Encrypt plaintext using XChaCha20-Poly1305.
///
/// # Preconditions
/// - `key` must be exactly KEY_LENGTH bytes
/// - `plaintext` can be any size
///
/// # Postconditions
/// - Returns nonce || ciphertext || tag
/// - The nonce is randomly generated
/// - The ciphertext length is plaintext length + TAG_SIZE + NONCE_SIZE
///
/// # Errors
/// - Returns error if key length is incorrect
/// - Returns error if encryption fails
///
/// # Security
/// - Uses random nonce generation
/// - Authenticates the ciphertext with Poly1305
pub fn encrypt(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    if key.len() != KEY_LENGTH {
        return Err(Error::Crypto(format!(
            "Invalid key length: expected {}, got {}",
            KEY_LENGTH,
            key.len()
        )));
    }

    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(key));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| Error::Crypto(format!("Encryption failed: {}", e)))?;

    // Prepend nonce to ciphertext
    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypt ciphertext using XChaCha20-Poly1305.
///
/// # Preconditions
/// - `key` must be exactly KEY_LENGTH bytes
/// - `ciphertext` must be at least NONCE_SIZE + TAG_SIZE bytes
/// - Ciphertext format: nonce || encrypted_data || tag
///
/// # Postconditions
/// - Returns the original plaintext
/// - Verifies authentication tag before returning
///
/// # Errors
/// - Returns error if key length is incorrect
/// - Returns error if ciphertext is too short
/// - Returns error if authentication fails (tampered data)
///
/// # Security
/// - Authenticates before decrypting
/// - Returns error on any authentication failure
pub fn decrypt(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    if key.len() != KEY_LENGTH {
        return Err(Error::Crypto(format!(
            "Invalid key length: expected {}, got {}",
            KEY_LENGTH,
            key.len()
        )));
    }

    if ciphertext.len() < NONCE_SIZE + TAG_SIZE {
        return Err(Error::Crypto("Ciphertext too short".to_string()));
    }

    let (nonce_bytes, encrypted) = ciphertext.split_at(NONCE_SIZE);
    let nonce = GenericArray::from_slice(nonce_bytes);

    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(key));

    cipher
        .decrypt(nonce, encrypted)
        .map_err(|e| Error::Crypto(format!("Decryption failed: {}", e)))
}

/// Encrypt plaintext with a specific nonce.
///
/// # Warning
/// This function should only be used when deterministic encryption is required
/// (e.g., for filename encryption). Using the same nonce twice with the same key
/// completely breaks security.
///
/// # Preconditions
/// - `nonce` must be unique for each (key, plaintext) pair
///
/// # Security
/// - Caller is responsible for nonce uniqueness
pub fn encrypt_with_nonce(key: &[u8], nonce: &[u8; NONCE_SIZE], plaintext: &[u8]) -> Result<Vec<u8>> {
    if key.len() != KEY_LENGTH {
        return Err(Error::Crypto(format!(
            "Invalid key length: expected {}, got {}",
            KEY_LENGTH,
            key.len()
        )));
    }

    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(key));
    let nonce_array = GenericArray::from_slice(nonce);

    cipher
        .encrypt(nonce_array, plaintext)
        .map_err(|e| Error::Crypto(format!("Encryption failed: {}", e)))
}

/// Decrypt ciphertext with a specific nonce.
pub fn decrypt_with_nonce(key: &[u8], nonce: &[u8; NONCE_SIZE], ciphertext: &[u8]) -> Result<Vec<u8>> {
    if key.len() != KEY_LENGTH {
        return Err(Error::Crypto(format!(
            "Invalid key length: expected {}, got {}",
            KEY_LENGTH,
            key.len()
        )));
    }

    if ciphertext.len() < TAG_SIZE {
        return Err(Error::Crypto("Ciphertext too short".to_string()));
    }

    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(key));
    let nonce_array = GenericArray::from_slice(nonce);

    cipher
        .decrypt(nonce_array, ciphertext)
        .map_err(|e| Error::Crypto(format!("Decryption failed: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Hello, World!";

        let ciphertext = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_ciphertext_size() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Test message";

        let ciphertext = encrypt(&key, plaintext).unwrap();

        // Size should be nonce + plaintext + tag
        assert_eq!(
            ciphertext.len(),
            NONCE_SIZE + plaintext.len() + TAG_SIZE
        );
    }

    #[test]
    fn test_different_nonce_each_time() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Same plaintext";

        let ct1 = encrypt(&key, plaintext).unwrap();
        let ct2 = encrypt(&key, plaintext).unwrap();

        // Nonces should be different
        assert_ne!(&ct1[..NONCE_SIZE], &ct2[..NONCE_SIZE]);
        // Ciphertexts should be different
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn test_wrong_key_fails() {
        let key1 = [1u8; KEY_LENGTH];
        let key2 = [2u8; KEY_LENGTH];
        let plaintext = b"Secret data";

        let ciphertext = encrypt(&key1, plaintext).unwrap();
        let result = decrypt(&key2, &ciphertext);

        assert!(result.is_err());
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Important data";

        let mut ciphertext = encrypt(&key, plaintext).unwrap();
        // Tamper with the ciphertext
        ciphertext[NONCE_SIZE + 5] ^= 0xFF;

        let result = decrypt(&key, &ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_key_length() {
        let short_key = [0u8; 16];
        let plaintext = b"data";

        assert!(encrypt(&short_key, plaintext).is_err());
    }

    #[test]
    fn test_encrypt_with_nonce() {
        let key = [42u8; KEY_LENGTH];
        let nonce = [1u8; NONCE_SIZE];
        let plaintext = b"Deterministic";

        let ct1 = encrypt_with_nonce(&key, &nonce, plaintext).unwrap();
        let ct2 = encrypt_with_nonce(&key, &nonce, plaintext).unwrap();

        // Same nonce should produce same ciphertext
        assert_eq!(ct1, ct2);

        // Should decrypt correctly
        let decrypted = decrypt_with_nonce(&key, &nonce, &ct1).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_empty_plaintext() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"";

        let ciphertext = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_large_plaintext() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = vec![0xABu8; 1_000_000]; // 1 MB

        let ciphertext = encrypt(&key, &plaintext).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }
}
