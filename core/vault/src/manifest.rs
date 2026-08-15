//! Authenticated snapshot generation manifest.

use std::collections::BTreeMap;

use axiomvault_common::{Error, Result, VaultId};
use axiomvault_crypto::{decrypt, encrypt, MasterKey};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const MANIFEST_FILENAME: &str = "generation.manifest";
const MANIFEST_KEY_CONTEXT: &[u8] = b"vault_generation_manifest_v1";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GenerationManifest {
    pub version: u32,
    pub vault_id: VaultId,
    pub generation: u64,
    pub tree_digest: String,
    pub config_digest: String,
    #[serde(default)]
    pub object_digests: BTreeMap<String, String>,
}

impl GenerationManifest {
    pub fn new(
        vault_id: VaultId,
        generation: u64,
        encrypted_tree: &[u8],
        config_bytes: &[u8],
        object_digests: BTreeMap<String, String>,
    ) -> Self {
        Self {
            version: 1,
            vault_id,
            generation,
            tree_digest: digest(encrypted_tree),
            config_digest: digest(config_bytes),
            object_digests,
        }
    }

    pub fn verify_bindings(
        &self,
        vault_id: &VaultId,
        encrypted_tree: &[u8],
        config_bytes: &[u8],
    ) -> Result<()> {
        if self.version != 1 || &self.vault_id != vault_id {
            return Err(Error::Crypto(
                "snapshot manifest vault identity mismatch".to_string(),
            ));
        }
        if self.tree_digest != digest(encrypted_tree) {
            return Err(Error::Crypto(
                "snapshot tree and manifest generations do not match".to_string(),
            ));
        }
        if self.config_digest != digest(config_bytes) {
            return Err(Error::Crypto(
                "vault config and snapshot manifest generations do not match".to_string(),
            ));
        }
        Ok(())
    }

    pub fn seal(&self, master_key: &MasterKey) -> Result<Vec<u8>> {
        let mut plaintext =
            serde_json::to_vec(self).map_err(|error| Error::Serialization(error.to_string()))?;
        let key = master_key.derive_file_key(MANIFEST_KEY_CONTEXT);
        let result = encrypt(key.as_bytes(), &plaintext);
        plaintext.zeroize();
        result
    }

    pub fn open(master_key: &MasterKey, bytes: &[u8]) -> Result<Self> {
        let key = master_key.derive_file_key(MANIFEST_KEY_CONTEXT);
        let mut plaintext = decrypt(key.as_bytes(), bytes)
            .map_err(|_| Error::Crypto("snapshot manifest authentication failed".to_string()))?;
        let result = serde_json::from_slice(&plaintext)
            .map_err(|error| Error::Serialization(error.to_string()));
        plaintext.zeroize();
        result
    }
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(bytes);
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}
