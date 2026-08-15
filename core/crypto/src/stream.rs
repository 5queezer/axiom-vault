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

/// Current header size: version (1) + chunk size (4).
pub const HEADER_SIZE: usize = 5;

/// Current stream encryption version.
pub const STREAM_VERSION: u8 = 2;
const LEGACY_STREAM_VERSION: u8 = 1;
const LEGACY_HEADER_REMAINDER: usize = 12;
const FRAME_METADATA_SIZE: usize = 13;
const FRAME_LENGTH_SIZE: usize = 4;
const MAX_CHUNK_SIZE: usize = 64 * 1024 * 1024;
const DATA_FRAME: u8 = 0;
const END_FRAME: u8 = 1;

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

    /// Encrypt from `reader` to `writer` with bounded memory.
    ///
    /// Version 2 consists of a five-byte header followed by length-prefixed,
    /// independently authenticated frames. Every frame authenticates its index,
    /// type, and the header chunk size. A mandatory authenticated end frame makes
    /// truncation detectable without knowing the input size in advance.
    pub fn encrypt_stream<R: Read, W: Write>(&self, mut reader: R, mut writer: W) -> Result<u64> {
        if self.chunk_size == 0 || self.chunk_size > MAX_CHUNK_SIZE {
            return Err(Error::Crypto("Invalid chunk size".to_string()));
        }
        let chunk_size = u32::try_from(self.chunk_size)
            .map_err(|_| Error::Crypto("Invalid chunk size".to_string()))?;
        writer.write_all(&[STREAM_VERSION])?;
        writer.write_all(&chunk_size.to_le_bytes())?;

        let mut buffer = vec![0u8; self.chunk_size];
        let mut index = 0u64;
        let mut total_bytes = 0u64;
        loop {
            let bytes_read = read_plaintext_chunk(&mut reader, &mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            self.write_frame(&mut writer, index, DATA_FRAME, &buffer[..bytes_read])?;
            total_bytes += bytes_read as u64;
            index += 1;
        }
        buffer.zeroize();
        self.write_frame(&mut writer, index, END_FRAME, &[])?;
        Ok(total_bytes)
    }

    fn write_frame<W: Write>(
        &self,
        writer: &mut W,
        index: u64,
        kind: u8,
        data: &[u8],
    ) -> Result<()> {
        let mut plaintext = Vec::with_capacity(FRAME_METADATA_SIZE + data.len());
        plaintext.extend_from_slice(&index.to_le_bytes());
        plaintext.push(kind);
        plaintext.extend_from_slice(&(self.chunk_size as u32).to_le_bytes());
        plaintext.extend_from_slice(data);
        let encrypted = encrypt(self.key, &plaintext)?;
        plaintext.zeroize();
        let frame_len = u32::try_from(encrypted.len())
            .map_err(|_| Error::Crypto("Encrypted frame too large".to_string()))?;
        writer.write_all(&frame_len.to_le_bytes())?;
        writer.write_all(&encrypted)?;
        Ok(())
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

    /// Decrypt data from reader and write authenticated plaintext to writer.
    /// Version 1 remains readable; all new ciphertext is emitted as version 2.
    pub fn decrypt_stream<R: Read, W: Write>(&self, mut reader: R, mut writer: W) -> Result<u64> {
        let mut version = [0u8; 1];
        reader.read_exact(&mut version)?;
        match version[0] {
            STREAM_VERSION => self.decrypt_v2(&mut reader, &mut writer),
            LEGACY_STREAM_VERSION => self.decrypt_v1(&mut reader, &mut writer),
            other => Err(Error::Crypto(format!(
                "Unsupported stream version: {}",
                other
            ))),
        }
    }

    fn decrypt_v2<R: Read, W: Write>(&self, reader: &mut R, writer: &mut W) -> Result<u64> {
        let mut chunk_size_bytes = [0u8; 4];
        reader.read_exact(&mut chunk_size_bytes)?;
        let chunk_size = u32::from_le_bytes(chunk_size_bytes) as usize;
        validate_chunk_size(chunk_size)?;

        let max_frame = NONCE_SIZE + TAG_SIZE + FRAME_METADATA_SIZE + chunk_size;
        let mut index = 0u64;
        let mut total_bytes = 0u64;
        loop {
            let mut length = [0u8; FRAME_LENGTH_SIZE];
            reader
                .read_exact(&mut length)
                .map_err(|_| Error::Crypto("Missing authenticated end frame".to_string()))?;
            let frame_len = u32::from_le_bytes(length) as usize;
            if frame_len < NONCE_SIZE + TAG_SIZE + FRAME_METADATA_SIZE || frame_len > max_frame {
                return Err(Error::Crypto("Invalid encrypted frame length".to_string()));
            }
            let mut encrypted = vec![0u8; frame_len];
            reader.read_exact(&mut encrypted)?;
            let mut plaintext = decrypt(self.key, &encrypted)?;
            if plaintext.len() < FRAME_METADATA_SIZE {
                plaintext.zeroize();
                return Err(Error::Crypto("Invalid frame metadata".to_string()));
            }
            let frame_index = u64::from_le_bytes(
                plaintext[..8]
                    .try_into()
                    .map_err(|_| Error::Crypto("Invalid frame index".to_string()))?,
            );
            let kind = plaintext[8];
            let authenticated_chunk_size = u32::from_le_bytes(
                plaintext[9..13]
                    .try_into()
                    .map_err(|_| Error::Crypto("Invalid authenticated chunk size".to_string()))?,
            ) as usize;
            if frame_index != index || authenticated_chunk_size != chunk_size {
                plaintext.zeroize();
                return Err(Error::Crypto("Stream metadata mismatch".to_string()));
            }
            match kind {
                DATA_FRAME if plaintext.len() > FRAME_METADATA_SIZE => {
                    writer.write_all(&plaintext[FRAME_METADATA_SIZE..])?;
                    total_bytes += (plaintext.len() - FRAME_METADATA_SIZE) as u64;
                    index += 1;
                    plaintext.zeroize();
                }
                END_FRAME if plaintext.len() == FRAME_METADATA_SIZE => {
                    plaintext.zeroize();
                    let mut trailing = [0u8; 1];
                    if reader.read(&mut trailing)? != 0 {
                        return Err(Error::Crypto("Trailing data after end frame".to_string()));
                    }
                    return Ok(total_bytes);
                }
                _ => {
                    plaintext.zeroize();
                    return Err(Error::Crypto("Invalid frame type".to_string()));
                }
            }
        }
    }

    fn decrypt_v1<R: Read, W: Write>(&self, reader: &mut R, writer: &mut W) -> Result<u64> {
        let mut header = [0u8; LEGACY_HEADER_REMAINDER];
        reader.read_exact(&mut header)?;
        let chunk_size = u32::from_le_bytes(
            header[..4]
                .try_into()
                .map_err(|_| Error::Crypto("Invalid legacy chunk size".to_string()))?,
        ) as usize;
        validate_chunk_size(chunk_size)?;
        let total_chunks = u64::from_le_bytes(
            header[4..]
                .try_into()
                .map_err(|_| Error::Crypto("Invalid legacy chunk count".to_string()))?,
        );
        let mut encrypted_buffer = vec![0u8; NONCE_SIZE + chunk_size + 8 + TAG_SIZE];
        let mut total_bytes = 0u64;
        for i in 0..total_chunks {
            let bytes_read = read_chunk(reader, &mut encrypted_buffer)?;
            if bytes_read == 0 {
                return Err(Error::Crypto("Unexpected end of stream".to_string()));
            }
            let mut decrypted = decrypt(self.key, &encrypted_buffer[..bytes_read])?;
            if decrypted.len() < 8 {
                decrypted.zeroize();
                return Err(Error::Crypto("Invalid chunk format".to_string()));
            }
            let chunk_index = u64::from_le_bytes(
                decrypted[..8]
                    .try_into()
                    .map_err(|_| Error::Crypto("Invalid chunk index".to_string()))?,
            );
            if chunk_index != i {
                decrypted.zeroize();
                return Err(Error::Crypto("Chunk order mismatch".to_string()));
            }
            writer.write_all(&decrypted[8..])?;
            total_bytes += (decrypted.len() - 8) as u64;
            decrypted.zeroize();
        }
        Ok(total_bytes)
    }
}

fn validate_chunk_size(chunk_size: usize) -> Result<()> {
    if chunk_size == 0 || chunk_size > MAX_CHUNK_SIZE {
        return Err(Error::Crypto(format!("Invalid chunk size: {}", chunk_size)));
    }
    Ok(())
}

fn read_plaintext_chunk<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<usize> {
    read_chunk(reader, buffer)
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

    /// A version-1 empty stream remains readable after the version-2 migration.
    #[test]
    fn test_zero_chunk_count_header() {
        let key = [42u8; KEY_LENGTH];
        let mut header = vec![LEGACY_STREAM_VERSION];
        header.extend_from_slice(&(DEFAULT_CHUNK_SIZE as u32).to_le_bytes());
        header.extend_from_slice(&0u64.to_le_bytes());
        let result = decrypt_bytes(&key, &header);
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

        assert_eq!(encrypted[0], STREAM_VERSION);
        let chunk_size = u32::from_le_bytes(encrypted[1..5].try_into().unwrap());
        assert_eq!(chunk_size as usize, DEFAULT_CHUNK_SIZE);
    }

    struct CountingReader {
        remaining: usize,
        consumed: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.remaining.min(buffer.len());
            buffer[..count].fill(0x5a);
            self.remaining -= count;
            self.consumed.set(self.consumed.get() + count);
            Ok(count)
        }
    }

    struct FirstWriteObserver {
        consumed: std::rc::Rc<std::cell::Cell<usize>>,
        total_input: usize,
        saw_write: bool,
    }

    impl Write for FirstWriteObserver {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.saw_write {
                assert!(
                    self.consumed.get() < self.total_input,
                    "encryption buffered the complete input before its first write"
                );
                self.saw_write = true;
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn encryption_writes_before_consuming_complete_large_input() {
        let key = [42u8; KEY_LENGTH];
        let total_input = DEFAULT_CHUNK_SIZE * 32;
        let consumed = std::rc::Rc::new(std::cell::Cell::new(0));
        let reader = CountingReader {
            remaining: total_input,
            consumed: consumed.clone(),
        };
        let writer = FirstWriteObserver {
            consumed,
            total_input,
            saw_write: false,
        };

        EncryptingStream::new(&key)
            .unwrap()
            .encrypt_stream(reader, writer)
            .unwrap();
    }

    #[test]
    fn tampered_middle_chunk_is_rejected() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = vec![0x3c; DEFAULT_CHUNK_SIZE * 3];
        let mut encrypted = encrypt_bytes(&key, &plaintext).unwrap();
        let middle = encrypted.len() / 2;
        encrypted[middle] ^= 0x80;

        assert!(decrypt_bytes(&key, &encrypted).is_err());
    }

    #[test]
    fn missing_authenticated_end_frame_is_rejected() {
        let key = [42u8; KEY_LENGTH];
        let plaintext = vec![0x3c; DEFAULT_CHUNK_SIZE * 2];
        let mut encrypted = encrypt_bytes(&key, &plaintext).unwrap();
        encrypted.truncate(encrypted.len() - (NONCE_SIZE + TAG_SIZE + 13));

        assert!(decrypt_bytes(&key, &encrypted).is_err());
    }
}
