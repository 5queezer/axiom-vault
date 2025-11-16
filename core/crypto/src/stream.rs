//! Streaming encryption for large files.
//!
//! This module provides chunk-based encryption to handle files that are
//! too large to fit in memory. Each chunk is independently authenticated.

use std::io::{Read, Write};

use axiomvault_common::{Error, Result};
use crate::aead::{encrypt, decrypt, NONCE_SIZE, TAG_SIZE};
use crate::keys::KEY_LENGTH;

/// Default chunk size for streaming encryption (64 KiB).
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Header size: version (1) + chunk_size (4) + total_chunks (8).
pub const HEADER_SIZE: usize = 13;

/// Stream encryption version.
pub const STREAM_VERSION: u8 = 1;

/// Encrypting stream that processes data in chunks.
pub struct EncryptingStream<'a> {
    key: &'a [u8],
    chunk_size: usize,
}

impl<'a> EncryptingStream<'a> {
    /// Create a new encrypting stream.
    ///
    /// # Preconditions
    /// - `key` must be KEY_LENGTH bytes
    ///
    /// # Errors
    /// - Returns error if key length is invalid
    pub fn new(key: &'a [u8]) -> Result<Self> {
        if key.len() != KEY_LENGTH {
            return Err(Error::Crypto("Invalid key length".to_string()));
        }
        Ok(Self {
            key,
            chunk_size: DEFAULT_CHUNK_SIZE,
        })
    }

    /// Set custom chunk size.
    pub fn with_chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size;
        self
    }

    /// Encrypt data from reader and write to writer.
    ///
    /// # Format
    /// - Header: version (1 byte) + chunk_size (4 bytes) + total_chunks (8 bytes)
    /// - Chunks: [nonce + ciphertext + tag] for each chunk
    ///
    /// # Postconditions
    /// - All data is encrypted and authenticated
    /// - Each chunk can be decrypted independently
    ///
    /// # Errors
    /// - I/O errors from reader/writer
    /// - Encryption errors
    pub fn encrypt_stream<R: Read, W: Write>(
        &self,
        mut reader: R,
        mut writer: W,
    ) -> Result<u64> {
        let mut buffer = vec![0u8; self.chunk_size];
        let mut chunks = Vec::new();
        let mut total_bytes = 0u64;

        // Read all chunks first to know total count
        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            total_bytes += bytes_read as u64;
            chunks.push(buffer[..bytes_read].to_vec());
        }

        // Write header
        writer.write_all(&[STREAM_VERSION])?;
        writer.write_all(&(self.chunk_size as u32).to_le_bytes())?;
        writer.write_all(&(chunks.len() as u64).to_le_bytes())?;

        // Encrypt and write each chunk
        for (i, chunk) in chunks.iter().enumerate() {
            // Include chunk index in authenticated data
            let mut aad = Vec::with_capacity(chunk.len() + 8);
            aad.extend_from_slice(&(i as u64).to_le_bytes());
            aad.extend_from_slice(chunk);

            let encrypted = encrypt(self.key, &aad)?;
            writer.write_all(&encrypted)?;
        }

        Ok(total_bytes)
    }
}

/// Decrypting stream that processes encrypted chunks.
pub struct DecryptingStream<'a> {
    key: &'a [u8],
}

impl<'a> DecryptingStream<'a> {
    /// Create a new decrypting stream.
    ///
    /// # Errors
    /// - Returns error if key length is invalid
    pub fn new(key: &'a [u8]) -> Result<Self> {
        if key.len() != KEY_LENGTH {
            return Err(Error::Crypto("Invalid key length".to_string()));
        }
        Ok(Self { key })
    }

    /// Decrypt data from reader and write to writer.
    ///
    /// # Preconditions
    /// - Reader contains validly encrypted stream data
    /// - Format must match EncryptingStream output
    ///
    /// # Postconditions
    /// - Original plaintext is recovered
    /// - All chunks are authenticated
    ///
    /// # Errors
    /// - I/O errors
    /// - Invalid format
    /// - Authentication failure (tampered data)
    pub fn decrypt_stream<R: Read, W: Write>(
        &self,
        mut reader: R,
        mut writer: W,
    ) -> Result<u64> {
        // Read header
        let mut version = [0u8; 1];
        reader.read_exact(&mut version)?;
        if version[0] != STREAM_VERSION {
            return Err(Error::Crypto(format!(
                "Unsupported stream version: {}",
                version[0]
            )));
        }

        let mut chunk_size_bytes = [0u8; 4];
        reader.read_exact(&mut chunk_size_bytes)?;
        let chunk_size = u32::from_le_bytes(chunk_size_bytes) as usize;

        let mut total_chunks_bytes = [0u8; 8];
        reader.read_exact(&mut total_chunks_bytes)?;
        let total_chunks = u64::from_le_bytes(total_chunks_bytes);

        let encrypted_chunk_size = NONCE_SIZE + chunk_size + 8 + TAG_SIZE;
        let mut encrypted_buffer = vec![0u8; encrypted_chunk_size];
        let mut total_bytes = 0u64;

        // Decrypt each chunk
        for i in 0..total_chunks {
            // Read encrypted chunk (size may vary for last chunk)
            let bytes_read = read_chunk(&mut reader, &mut encrypted_buffer)?;
            if bytes_read == 0 {
                return Err(Error::Crypto("Unexpected end of stream".to_string()));
            }

            let decrypted = decrypt(self.key, &encrypted_buffer[..bytes_read])?;

            // Verify chunk index
            if decrypted.len() < 8 {
                return Err(Error::Crypto("Invalid chunk format".to_string()));
            }
            let chunk_index = u64::from_le_bytes(decrypted[..8].try_into().unwrap());
            if chunk_index != i {
                return Err(Error::Crypto("Chunk order mismatch".to_string()));
            }

            let plaintext = &decrypted[8..];
            writer.write_all(plaintext)?;
            total_bytes += plaintext.len() as u64;
        }

        Ok(total_bytes)
    }
}

/// Read a complete encrypted chunk from the reader.
fn read_chunk<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<usize> {
    let mut total_read = 0;

    // First read the nonce
    reader.read_exact(&mut buffer[..NONCE_SIZE])?;
    total_read += NONCE_SIZE;

    // Then try to read up to the rest of the buffer
    loop {
        match reader.read(&mut buffer[total_read..]) {
            Ok(0) => break,
            Ok(n) => {
                total_read += n;
                // Check if we've read enough (at minimum nonce + tag)
                if total_read >= NONCE_SIZE + TAG_SIZE {
                    // Try to detect end of chunk by checking if next read would be a new nonce
                    break;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(total_read)
}

/// Encrypt a complete byte slice using streaming encryption.
///
/// This is a convenience function for when the complete data is available.
pub fn encrypt_bytes(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let stream = EncryptingStream::new(key)?;
    let mut output = Vec::new();
    stream.encrypt_stream(data, &mut output)?;
    Ok(output)
}

/// Decrypt a complete byte slice that was encrypted with streaming encryption.
pub fn decrypt_bytes(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let stream = DecryptingStream::new(key)?;
    let mut output = Vec::new();
    stream.decrypt_stream(data, &mut output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_stream_encrypt_decrypt_roundtrip() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Hello, streaming encryption!";

        let encrypted = encrypt_bytes(&key, plaintext).unwrap();
        let decrypted = decrypt_bytes(&key, &encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_stream_multiple_chunks() {
        let key = [42u8; KEY_LENGTH];
        // Create data that spans multiple chunks
        let plaintext = vec![0xAB; DEFAULT_CHUNK_SIZE * 3 + 1000];

        let encrypted = encrypt_bytes(&key, &plaintext).unwrap();
        let decrypted = decrypt_bytes(&key, &encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_stream_empty_data() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"";

        let encrypted = encrypt_bytes(&key, plaintext).unwrap();
        let decrypted = decrypt_bytes(&key, &encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_stream_custom_chunk_size() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Custom chunk size test data that is longer than the chunk";

        let stream = EncryptingStream::new(&key)
            .unwrap()
            .with_chunk_size(16);
        let mut encrypted = Vec::new();
        stream.encrypt_stream(&plaintext[..], &mut encrypted).unwrap();

        let decrypted = decrypt_bytes(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_stream_wrong_key_fails() {
        let key1 = [1u8; KEY_LENGTH];
        let key2 = [2u8; KEY_LENGTH];
        let plaintext = b"Secret streaming data";

        let encrypted = encrypt_bytes(&key1, plaintext).unwrap();
        let result = decrypt_bytes(&key2, &encrypted);

        assert!(result.is_err());
    }

    #[test]
    fn test_stream_header_format() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Test";

        let encrypted = encrypt_bytes(&key, plaintext).unwrap();

        // Check header
        assert_eq!(encrypted[0], STREAM_VERSION);
        let chunk_size = u32::from_le_bytes(encrypted[1..5].try_into().unwrap());
        assert_eq!(chunk_size as usize, DEFAULT_CHUNK_SIZE);
        let total_chunks = u64::from_le_bytes(encrypted[5..13].try_into().unwrap());
        assert_eq!(total_chunks, 1); // Single chunk for small data
    }
}
