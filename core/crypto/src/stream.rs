//! Streaming encryption for large files.
//!
//! This module provides chunk-based encryption to handle files that are
//! too large to fit in memory. Each chunk is independently authenticated.

use std::io::{Read, Write};

use zeroize::Zeroize;

use crate::aead::{decrypt, encrypt, NONCE_SIZE, TAG_SIZE};
use crate::keys::KEY_LENGTH;
use axiomvault_common::{Error, Result};

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
    /// - Chunks: nonce (24 B) || encrypt(index_le64 || plaintext) || tag (16 B)
    ///
    /// The chunk index is prepended to the plaintext (and therefore authenticated
    /// by Poly1305) to detect chunk reordering or injection attacks.
    ///
    /// # Known limitation
    /// The current implementation reads all encrypted chunks into a `Vec` before
    /// writing, because `total_chunks` is written in the header and cannot be
    /// known until all chunks are processed. For large files this doubles
    /// peak memory usage. A future revision should either write chunks to a
    /// temporary file and seek back to fill in the header, or remove the
    /// `total_chunks` field from the header (using EOF instead).
    ///
    /// # Postconditions
    /// - All data is encrypted and authenticated
    /// - Chunk ordering is verified on decryption
    ///
    /// # Errors
    /// - I/O errors from reader/writer
    /// - Encryption errors
    pub fn encrypt_stream<R: Read, W: Write>(&self, mut reader: R, mut writer: W) -> Result<u64> {
        let mut buffer = vec![0u8; self.chunk_size];
        let mut encrypted_chunks: Vec<Vec<u8>> = Vec::new();
        let mut total_bytes = 0u64;

        // Encrypt each chunk as it arrives; store encrypted output until we know
        // the total count (needed for the header).
        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            let chunk_index = encrypted_chunks.len() as u64;
            total_bytes += bytes_read as u64;

            // Prepend chunk index to the plaintext so it is authenticated.
            let mut plaintext = Vec::with_capacity(8 + bytes_read);
            plaintext.extend_from_slice(&chunk_index.to_le_bytes());
            plaintext.extend_from_slice(&buffer[..bytes_read]);

            let encrypted = encrypt(self.key, &plaintext)?;
            plaintext.zeroize();
            encrypted_chunks.push(encrypted);
        }

        buffer.zeroize();

        // Write header
        writer.write_all(&[STREAM_VERSION])?;
        writer.write_all(&(self.chunk_size as u32).to_le_bytes())?;
        writer.write_all(&(encrypted_chunks.len() as u64).to_le_bytes())?;

        // Write encrypted chunks
        for chunk in encrypted_chunks {
            writer.write_all(&chunk)?;
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
    pub fn decrypt_stream<R: Read, W: Write>(&self, mut reader: R, mut writer: W) -> Result<u64> {
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

        // Validate chunk size to prevent malicious headers causing huge allocations (e.g. 4GB)
        const MAX_CHUNK_SIZE: usize = 64 * 1024 * 1024; // 64 MiB
        if chunk_size > MAX_CHUNK_SIZE {
            return Err(Error::Crypto(format!(
                "Chunk size {} exceeds maximum allowed ({} bytes)",
                chunk_size, MAX_CHUNK_SIZE
            )));
        }

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

            let mut decrypted = decrypt(self.key, &encrypted_buffer[..bytes_read])?;

            // Verify chunk index
            if decrypted.len() < 8 {
                decrypted.zeroize();
                return Err(Error::Crypto("Invalid chunk format".to_string()));
            }
            let chunk_index = u64::from_le_bytes(decrypted[..8].try_into().unwrap());
            if chunk_index != i {
                decrypted.zeroize();
                return Err(Error::Crypto("Chunk order mismatch".to_string()));
            }

            let plaintext = &decrypted[8..];
            writer.write_all(plaintext)?;
            total_bytes += plaintext.len() as u64;
            decrypted.zeroize();
        }

        Ok(total_bytes)
    }
}

/// Read a complete encrypted chunk from the reader.
///
/// Reads as many bytes as possible into `buffer`, returning the count.
/// Returns 0 only if the reader is immediately at EOF (no data for this chunk).
///
/// For all but the last chunk the buffer will be filled completely.
/// For the last (partial) chunk fewer bytes are returned — the caller must
/// pass only `buffer[..bytes_read]` to the decryption function.
///
/// Note: `Read::read` may return partial data on a single call (this is
/// legal for file/network I/O). We loop until the buffer is full or we hit
/// EOF, correctly handling short reads.
fn read_chunk<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<usize> {
    let mut total_read = 0;

    while total_read < buffer.len() {
        match reader.read(&mut buffer[total_read..]) {
            Ok(0) => break, // EOF
            Ok(n) => total_read += n,
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
    use proptest::prelude::*;

    proptest! {
        /// Property: encrypt then decrypt always roundtrips for arbitrary data.
        #[test]
        fn stream_roundtrip_arbitrary_data(data in prop::collection::vec(any::<u8>(), 0..4096)) {
            let key = [42u8; KEY_LENGTH];
            let encrypted = encrypt_bytes(&key, &data).unwrap();
            let decrypted = decrypt_bytes(&key, &encrypted).unwrap();
            prop_assert_eq!(decrypted, data);
        }

        /// Property: encrypt then decrypt roundtrips with various chunk sizes.
        #[test]
        fn stream_roundtrip_various_chunk_sizes(
            data in prop::collection::vec(any::<u8>(), 1..2048),
            chunk_size in 1usize..512,
        ) {
            let key = [7u8; KEY_LENGTH];
            let stream = EncryptingStream::new(&key).unwrap().with_chunk_size(chunk_size);
            let mut encrypted = Vec::new();
            stream.encrypt_stream(&data[..], &mut encrypted).unwrap();

            let decrypted = decrypt_bytes(&key, &encrypted).unwrap();
            prop_assert_eq!(decrypted, data);
        }
    }

    /// Truncated ciphertext must return an error, never panic.
    #[test]
    fn test_truncated_ciphertext_returns_error() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Sensitive data for truncation test";
        let encrypted = encrypt_bytes(&key, plaintext).unwrap();

        // Truncate at various points
        for truncate_at in [
            0,
            1,
            5,
            HEADER_SIZE - 1,
            HEADER_SIZE,
            HEADER_SIZE + 1,
            encrypted.len() / 2,
            encrypted.len() - 1,
        ] {
            if truncate_at >= encrypted.len() {
                continue;
            }
            let truncated = &encrypted[..truncate_at];
            let result = decrypt_bytes(&key, truncated);
            assert!(
                result.is_err(),
                "Expected error for truncation at byte {}",
                truncate_at
            );
        }
    }

    /// Corrupted chunk data must return an error, never panic.
    #[test]
    fn test_corrupted_chunk_data_returns_error() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Data to be corrupted in chunk body";
        let encrypted = encrypt_bytes(&key, plaintext).unwrap();

        // Corrupt a byte in the chunk body (after the header)
        if encrypted.len() > HEADER_SIZE + 5 {
            let mut corrupted = encrypted.clone();
            corrupted[HEADER_SIZE + 5] ^= 0xFF;
            let result = decrypt_bytes(&key, &corrupted);
            assert!(result.is_err(), "Expected error for corrupted chunk data");
        }
    }

    /// Invalid version byte in header must return an error.
    #[test]
    fn test_invalid_version_returns_error() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Version check data";
        let mut encrypted = encrypt_bytes(&key, plaintext).unwrap();

        // Set version to unsupported value
        encrypted[0] = 99;
        let result = decrypt_bytes(&key, &encrypted);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unsupported stream version"),
            "Expected version error, got: {}",
            err_msg
        );
    }

    /// Bad chunk count (higher than actual data) must return an error.
    #[test]
    fn test_bad_chunk_count_returns_error() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = b"Chunk count test";
        let mut encrypted = encrypt_bytes(&key, plaintext).unwrap();

        // Set total_chunks to a large number (header bytes 5..13)
        let bogus_count: u64 = 9999;
        encrypted[5..13].copy_from_slice(&bogus_count.to_le_bytes());
        let result = decrypt_bytes(&key, &encrypted);
        assert!(result.is_err(), "Expected error for bad chunk count");
    }

    /// Empty input (zero bytes) must be handled gracefully.
    #[test]
    fn test_empty_input_decrypt_returns_error() {
        let key = [42u8; KEY_LENGTH];
        let result = decrypt_bytes(&key, &[]);
        assert!(result.is_err(), "Expected error for empty ciphertext input");
    }

    /// Header-only input (no chunk data, but claims 1 chunk) must error.
    #[test]
    fn test_header_only_with_nonzero_chunks_returns_error() {
        let key = [42u8; KEY_LENGTH];
        let mut header = vec![STREAM_VERSION];
        header.extend_from_slice(&(DEFAULT_CHUNK_SIZE as u32).to_le_bytes());
        header.extend_from_slice(&1u64.to_le_bytes()); // claims 1 chunk but no data follows
        let result = decrypt_bytes(&key, &header);
        assert!(
            result.is_err(),
            "Expected error for header-only input with nonzero chunk count"
        );
    }

    /// Chunk size of zero in header should be handled (decrypt of empty chunks).
    #[test]
    fn test_zero_chunk_count_header() {
        let key = [42u8; KEY_LENGTH];
        let mut header = vec![STREAM_VERSION];
        header.extend_from_slice(&(DEFAULT_CHUNK_SIZE as u32).to_le_bytes());
        header.extend_from_slice(&0u64.to_le_bytes()); // 0 chunks
        let result = decrypt_bytes(&key, &header);
        // 0 chunks means empty file — should succeed with empty output
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

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

        let stream = EncryptingStream::new(&key).unwrap().with_chunk_size(16);
        let mut encrypted = Vec::new();
        stream
            .encrypt_stream(&plaintext[..], &mut encrypted)
            .unwrap();

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
