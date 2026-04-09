# Security Policy

## Reporting Vulnerabilities

If you discover a security vulnerability in AxiomVault, please report it responsibly.

**Email:** [security@axiomvault.dev](mailto:security@axiomvault.dev)

If possible, encrypt your report using our PGP key (available on request). Include:

- A description of the vulnerability and its impact
- Steps to reproduce or a proof-of-concept
- The version of AxiomVault affected

We will acknowledge your report within **48 hours** and aim to provide a fix or mitigation plan within 7 days for critical issues.

There is no bug bounty program at this time. We will credit reporters in release notes unless you prefer to remain anonymous.

## Threat Model

### What we protect against

- **Cloud provider compromise.** The storage provider sees only encrypted blobs. File contents, file names, directory structure, and metadata are encrypted client-side with XChaCha20-Poly1305 (AEAD). The tree index is encrypted before upload.

- **Disk attacker (stolen device).** The vault master key is never stored in plaintext. It is wrapped (encrypted) under a key-encryption key (KEK) derived from the user's password via Argon2id with a per-vault random salt. Sensitive types in memory derive `Zeroize`/`ZeroizeOnDrop` and are wiped when they go out of scope.

- **Network eavesdropper.** All cloud API traffic uses TLS (via `rustls`). The local FUSE mount is process-local and does not expose a network service.

- **Partial cloud data loss.** Cloud RAID modes (mirror and erasure coding via Reed-Solomon) replicate data across multiple storage providers for redundancy.

### What we do NOT protect against

- **Local malware with root/admin access.** An attacker with full system access can read process memory, inject code, or intercept FUSE operations. This is out of scope for AxiomVault.

- **Memory forensics on a running system.** We zeroize sensitive data eagerly, but we cannot guarantee all copies are erased. The compiler may optimize away zeroization in some cases, and secrets may leak to swap or core dumps. Use full-disk encryption as defense in depth.

- **Rubber-hose cryptanalysis.** Physical coercion is out of scope.

- **Side-channel attacks on shared hardware.** We use constant-time comparison via the `subtle` crate for sensitive equality checks, but we do not claim full side-channel resistance on shared cloud VMs or in the presence of speculative execution attacks.

### Assumptions

- The user's device is not compromised at the time of vault creation or unlock.
- Rust's memory safety guarantees hold (no undefined behavior outside `unsafe` blocks, and all `unsafe` blocks have documented safety invariants).
- The underlying cryptographic primitives (XChaCha20-Poly1305, Argon2id, Blake2b) are sound.
- The OS kernel and FUSE subsystem correctly enforce mount permissions.

## Cryptographic Design

| Component | Algorithm | Details |
|---|---|---|
| Encryption | XChaCha20-Poly1305 | 256-bit key, 192-bit random nonce, AEAD |
| Password KDF | Argon2id | Configurable memory/time/parallelism; defaults target 0.5-1s |
| Non-KDF hashing | Blake2b | Used for file/directory key derivation and recovery KEK derivation |
| Recovery key encoding | BIP39 mnemonic | 24 words encoding 256 bits of entropy |

### Key hierarchy

```text
User password
    |
    v
Argon2id(password, salt) --> Password KEK --> wraps Master Key
                                                  |
                                          derives file/dir keys
                                          (Blake2b with domain separation)

Recovery key (24 BIP39 words, shown once)
    |
    v
Blake2b(entropy, context) --> Recovery KEK --> wraps same Master Key
```

The master key is randomly generated and stored only in wrapped (encrypted) form. Two independent KEKs can unwrap it: one from the password, one from the recovery mnemonic. The recovery key uses Blake2b (not Argon2id) because it is already 256 bits of high-entropy randomness.

### File encryption

Files are encrypted using chunked streaming encryption (default 64 KiB chunks). Each chunk is independently authenticated with XChaCha20-Poly1305 using a per-chunk random nonce. Chunk indices are included in the authenticated data to prevent reordering or truncation.

## Security Practices

- **Dependency auditing.** `cargo audit` runs in CI on every push. Dependencies are checked for known vulnerabilities.
- **Secret scanning.** `gitleaks` runs as a pre-commit hook and in CI to prevent accidental credential commits.
- **Lint enforcement.** `cargo clippy -D warnings` is enforced in CI and pre-commit hooks.
- **Unsafe discipline.** Every `unsafe` block requires a `// SAFETY:` comment explaining the invariant. This is enforced by a pre-commit hook.
- **Memory hygiene.** All types holding key material, plaintext buffers, or passwords derive `Zeroize` and `ZeroizeOnDrop`. Debug implementations on key types print `[REDACTED]`.
- **Constant-time comparisons.** Sensitive equality checks (e.g., recovery key verification) use the `subtle` crate's `ConstantTimeEq`.

## Supported Versions

Only the latest release receives security patches. We recommend always running the most recent version.

| Version | Supported |
|---|---|
| Latest | Yes |
| Older | No |
