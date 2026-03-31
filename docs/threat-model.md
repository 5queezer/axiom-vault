# AxiomVault Threat Model

## Overview

AxiomVault is a client-side encrypted vault for files and directories. All
cryptographic operations happen locally; cloud storage providers see only
opaque ciphertext.

## Assets

| Asset | Sensitivity | Location |
|---|---|---|
| User files (plaintext) | High | Memory while vault is unlocked |
| Master key | Critical | Memory while vault is unlocked |
| Password-derived KEK | Critical | Memory during unlock/lock |
| Recovery key | Critical | Shown once, never stored in plaintext |
| Vault tree (filenames, structure) | Medium | Encrypted on storage; in memory while unlocked |
| Encrypted blobs | Low | Storage backend |
| Vault config (salt, KDF params, wrapped keys) | Low | Storage backend |

## Threat Actors

### T1 -- Storage-level attacker

**Capability:** Read (and possibly modify) all data on the storage backend
(local disk, Dropbox, Google Drive, OneDrive).

**Mitigations:**
- All file content encrypted with XChaCha20-Poly1305 (per-file keys).
- Filenames and directory structure encrypted.
- AEAD authentication detects tampering.
- Master key wrapped with Argon2id-derived KEK (password) and Blake2b-derived
  KEK (recovery key).

**Residual risk:** File sizes, number of files, and modification timestamps
are visible. An attacker can observe when files change but not what changed.

### T2 -- Network observer

**Capability:** Observe sync traffic between client and storage provider.

**Mitigations:**
- All provider communication over TLS.
- Payload is already ciphertext.

**Residual risk:** Traffic analysis reveals sync timing, file count, and
approximate file sizes. No padding or dummy traffic is applied.

### T3 -- Local attacker (unprivileged)

**Capability:** Read files owned by other users on the same machine.

**Mitigations:**
- Vault config and local index databases created with mode 0600.
- FUSE mount defaults to `SessionACL::Owner`.

**Residual risk:** If the vault is mounted with `allow_other`, other local
users can access decrypted content through the FUSE mount.

### T4 -- Local attacker (privileged / code execution)

**Capability:** Read process memory, attach debugger, inspect swap.

**Mitigations:**
- Key material zeroized on drop (best-effort via `zeroize` crate).
- Intermediate plaintext buffers wiped after use where practical.
- No plaintext vault paths or filenames in default log output.

**Residual risk:** A privileged attacker can read decrypted keys and content
from process memory. The compiler may spill sensitive data to stack or
registers beyond our control. Swap and core dumps may contain key material.
This is **out of scope** for AxiomVault's threat model.

### T5 -- Password brute-force

**Capability:** Offline dictionary/brute-force attack against the vault config.

**Mitigations:**
- Argon2id with configurable cost parameters (default: `moderate` preset).
- Random 256-bit salt per vault.
- Key verification via AEAD decryption of a known constant (not a hash).

**Residual risk:** Weak passwords remain vulnerable regardless of KDF cost.

## Logging Policy

AxiomVault does **not** log plaintext vault paths, filenames, or file content
in default builds. Log output includes:
- Operation type (create, read, update, delete, sync)
- Numeric identifiers (inode numbers, file handles, change IDs)
- Error messages (without path context)
- File sizes (byte counts only)

Mount point paths (host filesystem, not vault-internal) are logged at `info`
level because they are operationally necessary and not vault secrets.

## Cryptographic Primitives

| Purpose | Algorithm | Notes |
|---|---|---|
| File/tree encryption | XChaCha20-Poly1305 | 192-bit nonce, 256-bit key |
| Key derivation (password) | Argon2id | Configurable time/memory cost |
| Key derivation (recovery) | Blake2b-256 | High-entropy input, no slow KDF needed |
| Key derivation (file/dir keys) | Blake2b-256 | Domain-separated from master key |
| Random generation | OS CSPRNG via `rand` | Used for keys, salts, nonces |
| Constant-time comparison | `subtle` crate | Password verification, recovery verification |

## Out of Scope

- Protection against a compromised OS kernel or hypervisor.
- Side-channel attacks (timing, power analysis, speculative execution).
- Deniable encryption or hidden volumes.
- Multi-user access control within a single vault.
- Forward secrecy (key rotation is manual via password change).
