//! Cryptographic primitives for AxiomVault.
//!
//! This module provides:
//! - Key derivation using Argon2id
//! - Authenticated encryption using XChaCha20-Poly1305
//! - Secure key management with automatic zeroization
//! - Streaming encryption for large files
//!
//! # Security Guarantees
//! - All key material is automatically zeroized on drop
//! - No plaintext or key material is ever logged
//! - Constant-time operations for sensitive comparisons

pub mod kdf;
pub mod aead;
pub mod keys;
pub mod stream;

pub use aead::{decrypt, encrypt};
pub use kdf::{derive_key, KdfParams};
pub use keys::{MasterKey, FileKey, DirectoryKey, Salt};
pub use stream::{EncryptingStream, DecryptingStream};
