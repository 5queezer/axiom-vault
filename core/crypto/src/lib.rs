//! Cryptographic primitives for AxiomVault.
//!
//! This module provides:
//! - Key derivation using Argon2id
//! - Authenticated encryption using XChaCha20-Poly1305
//! - Secure key management with automatic zeroization
//! - Streaming encryption for large files
//!
//! # Security notes
//! - Key types implement zeroization on drop. Intermediate buffers are wiped on a best-effort basis.
//! - Sensitive comparisons use constant-time equality where applicable.
//! - Logging policy is enforced at higher layers. Avoid logging plaintext paths or secrets.

pub mod aead;
pub mod kdf;
pub mod keys;
pub mod recovery;
pub mod stream;

pub use aead::{decrypt, encrypt};
pub use kdf::{derive_key, KdfParams};
pub use keys::{DirectoryKey, FileKey, MasterKey, Salt};
pub use recovery::RecoveryKey;
pub use stream::{DecryptingStream, EncryptingStream};
