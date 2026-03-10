# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in AxiomVault, please report it
**privately** to avoid exposing users to risk before a fix is available.

- **Email:** <christian.pojoni@gmail.com>
- **GitHub:** Open a [private security advisory](https://github.com/5queezer/axiom-vault/security/advisories/new)

Please include:
1. A description of the vulnerability and its impact.
2. Steps to reproduce.
3. Affected versions, if known.

We aim to acknowledge reports within 48 hours and provide a fix or mitigation
within 14 days for critical issues.

## Supported Versions

Only the latest release on the `master` branch receives security updates.

## Security Design

See [`docs/threat-model.md`](docs/threat-model.md) for the full threat model.

### What we defend against

- **At-rest confidentiality:** vault files are encrypted with XChaCha20-Poly1305
  under per-file keys derived from a random master key. Directory structure and
  filenames are encrypted. An attacker with read access to the storage backend
  cannot recover plaintext content, filenames, or directory structure without
  the password or recovery key.

- **Password brute-force:** the master key is wrapped with a key-encryption key
  derived from the user password via Argon2id with tunable cost parameters.

- **Key compromise scope:** each vault has an independent random master key.
  Compromise of one vault does not affect others.

### What we do *not* defend against

- **Compromised host:** if an attacker has code execution on the machine where
  the vault is unlocked, they can read decrypted content from memory. Zeroization
  of key material is best-effort and cannot prevent a privileged attacker from
  reading process memory.

- **Side-channel attacks:** we use constant-time comparisons where practical, but
  do not claim resistance to power analysis, cache-timing, or speculative
  execution attacks.

- **Traffic analysis:** an attacker observing sync traffic can see file sizes,
  access patterns, and timing. We do not pad ciphertext or add dummy traffic.

### Zeroization disclaimer

Key types (`MasterKey`, `FileKey`, `DirectoryKey`, `RecoveryKey`) implement
`ZeroizeOnDrop`. Intermediate buffers (KDF output, unwrapped key material,
decrypted tree metadata) are wiped on a best-effort basis using the `zeroize`
crate. The Rust compiler or OS may copy sensitive data to locations we cannot
control (stack spills, swap, core dumps). For high-assurance environments,
consider OS-level mitigations such as `mlock`, disabled swap, and restricted
core dumps.
