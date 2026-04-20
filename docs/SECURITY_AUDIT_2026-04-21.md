# AxiomVault Security Audit — 2026-04-21

**Auditor:** Claude (Opus 4.7) using `cybersecurity-skills` plugin (commit `72a2cd6`).
**Scope:** `core/crypto`, `core/vault`, `core/storage`, `core/sync`, `core/ffi`, `core/fuse`, dependency tree.
**Out of scope:** `clients/*` (Linux GTK, Apple SwiftUI, Android Kotlin), `tools/cli`, `core/webdav` (only inspected where it intersects scope), `core/common`.
**Method:** Static source review + `cargo audit` + `cargo deny check` + targeted greps for unsafe blocks, panic surfaces, secret literals.

---

## Severity legend

- **CRITICAL** — exploitable now, breaks confidentiality/integrity, fix immediately.
- **HIGH** — exploitable under realistic conditions, or correctness gap that silently loses data.
- **MEDIUM** — defense-in-depth gap, hardening issue, or exploitable only under non-default config.
- **LOW** — code-quality / future-proofing issue with no current exploit path.
- **INFO** — observation, no action required.

---

## Summary

| ID  | Severity | Component | Title                                                                  |
| --- | -------- | --------- | ---------------------------------------------------------------------- |
| C-1 | CRITICAL | deps      | `rustls-webpki 0.103.10` — name-constraint bypass (RUSTSEC-2026-0098/9) |
| H-1 | HIGH     | sync      | `download_remote_changes` discards downloaded data                     |
| H-2 | HIGH     | ffi       | Recovery mnemonic exits over FFI as plain `String`/`CString` — never zeroized |
| H-3 | HIGH     | ffi       | Passwords cloned into `String` at FFI boundary, never zeroized          |
| M-1 | MEDIUM   | deps      | `rand 0.10.0` (and 0.8/0.9 transitives) — RUSTSEC-2026-0097 unsoundness |
| M-2 | MEDIUM   | storage   | OAuth desktop client uses `client_secret` instead of PKCE              |
| M-3 | MEDIUM   | storage   | OAuth CSRF verification not enforced at the library layer              |
| M-4 | MEDIUM   | storage   | `LocalProvider` uses default umask — vault config + ciphertext world-readable on Linux |
| M-5 | MEDIUM   | sync      | Plaintext-capable staging area writes data with default umask           |
| M-6 | MEDIUM   | sync      | `generate_conflict_path` second-resolution timestamp races              |
| M-7 | MEDIUM   | crypto    | `EncryptingStream` buffers entire encrypted output in RAM               |
| M-8 | MEDIUM   | fuse      | `open()` reads entire decrypted file into RAM                           |
| M-9 | MEDIUM   | fuse      | Two `unsafe { libc::getuid()/getgid() }` calls without `// SAFETY:` comments |
| M-10| MEDIUM   | vault     | Migration framework (`MigrationRegistry`) only works for local-disk vaults |
| L-1 | LOW      | crypto    | `encrypt_with_nonce` is a public footgun with no programmatic safeguard |
| L-2 | LOW      | crypto    | `Salt` field is `pub` — externally mutable                              |
| L-3 | LOW      | crypto    | `MasterKey::from_bytes` accepts `[u8; 32]` by value — caller copy not zeroized |
| L-4 | LOW      | crypto    | `decrypted[..8].try_into().unwrap()` in `stream::decrypt_stream` lib code |
| L-5 | LOW      | storage   | `build_http_client` has no total request timeout — DoS / hang risk      |
| L-6 | LOW      | storage   | CRC-32 in erasure shards is non-cryptographic (defense-in-depth only)   |
| L-7 | LOW      | sync      | `StagingArea::new` silently discards corrupted registry                  |
| L-8 | LOW      | sync      | Retry executor doesn't refresh OAuth tokens on Authentication errors    |
| L-9 | LOW      | fuse      | `open()` ignores `O_RDONLY/O_WRONLY/O_RDWR` flags                       |
| L-10| LOW      | cli       | OAuth CSRF token compared with `!=` (non-constant-time)                 |
| I-1 | INFO     | crypto    | Argon2id `interactive` profile is conservative (64 MiB / 3 iter)        |
| I-2 | INFO     | vault     | `MigrationV1_0ToV1_1` is a no-op placeholder; real migration lives in `VaultConfig::migrate_to_v1_1` |

---

## Findings

### C-1 — `rustls-webpki 0.103.10` name-constraint bypass (RUSTSEC-2026-0098 + 0099)

**Severity:** CRITICAL. **Component:** transitive dependency.
**Reachability:** Every cloud storage call (Google Drive, Dropbox, OneDrive, iCloud) traverses `reqwest → hyper-rustls → rustls → rustls-webpki`.

Two related advisories from 2026-04-14:

- **RUSTSEC-2026-0098** — name constraints for URI names incorrectly accepted.
- **RUSTSEC-2026-0099** — name constraints accepted for certificates asserting a wildcard name.

Both are misissuance-bypass bugs in `rustls-webpki ≤ 0.103.11`. AxiomVault relies on TLS to authenticate Google/Dropbox/OneDrive endpoints; a constrained intermediate CA could be tricked into accepting a cert outside its constraint.

**Fix:** `cargo update -p rustls-webpki` (target version `>= 0.103.12`).

After upgrade, re-run `cargo audit` to confirm. Pin in `Cargo.toml` if needed.

---

### H-1 — Sync engine discards downloaded remote data

**Severity:** HIGH. **File:** `core/sync/src/engine.rs:520-547`.

```rust
match download_result {
    Ok(_data) => {
        // In a real implementation, we would write this to the vault
        // For now, just update the state
        ...
```

`download_remote_changes` downloads the ciphertext, then immediately drops `_data` and only updates the etag/timestamp in the in-memory sync state. **Remote-side changes are silently lost** — the local vault never sees them.

**Fix:** Pipe `_data` into the vault's storage layer (write to local provider / staging area for a future commit, or directly invoke `VaultOperations::update_file` with the decrypted plaintext if applicable).

This is a correctness gap that *masquerades* as success in the sync result counters, which makes it harder to detect operationally.

---

### H-2 — Recovery mnemonic leaves FFI without zeroization

**Severity:** HIGH. **Files:** `core/ffi/src/lib.rs:441-462`, `core/ffi/src/vault_ops.rs:56`, `core/ffi/src/vault_ops.rs:206-209`.

The recovery key is 256 bits of master-key-equivalent entropy. The vault crate correctly wraps it in `Zeroizing<String>` (`recovery.rs:62`). But at the FFI boundary it is converted to a plain `String`:

```rust
// vault_ops.rs:56
recovery_words: std::sync::Mutex::new(Some(String::from(&*result.recovery_words))),
```

```rust
// lib.rs:456
Some(w) => CString::new(w).map(|s| s.into_raw())  // heap-allocated copy, dropped without zeroize
```

The `Zeroizing` wrapper is dropped at the boundary; the resulting `String` and `CString` heap allocations are freed without being wiped. Same pattern in `axiom_vault_show_recovery_key` (`lib.rs:478`) and `vault_ops::show_recovery_key` (`vault_ops.rs:206-209`).

**Why it matters:** anyone who can read process memory after a vault was created/recovered (core dump, swap file, debugger, attached profiler) can reconstruct the master key.

**Fix:**
- Hold `Zeroizing<String>` end-to-end inside `FFIVaultHandle.recovery_words`.
- Write a small helper that copies straight from `Zeroizing<String>` into a heap buffer the FFI caller will free, and zeroize the source after the copy.
- For `axiom_string_free` of recovery output specifically, zeroize the bytes before `CString::from_raw` drops them, *or* expose a separate `axiom_recovery_words_free` that zeroizes.

---

### H-3 — Passwords are heap-`String`-cloned across the FFI boundary

**Severity:** HIGH. **Files:** `core/ffi/src/vault_ops.rs:46, 70, 178, 222`.

Every password-taking entry point converts the C string into `&str`, then immediately calls `.to_string()` to build `CreateVaultParams { password: password.to_string(), ... }` etc. The resulting `String` lives on the heap until `CreateVaultParams` is dropped, with no zeroization.

Cumulative effect across `create_vault`, `open_vault`, `change_password` (twice — old + new), and `reset_password`: up to 5 password copies per session, each persisting on the allocator's freelist after drop.

**Fix:** make `*Params` structs hold `Zeroizing<String>` (or `SecretString`/`secrecy` crate) for password fields, and propagate that all the way down to `derive_key`.

---

### M-1 — `rand` unsoundness (RUSTSEC-2026-0097)

**Severity:** MEDIUM. **Versions present:** 0.8.5, 0.9.2, 0.10.0.

> Rand is unsound with a custom logger using `rand::rng()`.

AxiomVault uses `rand::rng().fill(...)` in `crypto/keys.rs`, `crypto/recovery.rs`, and `sync/retry.rs`. Exploitability requires a custom `tracing`/`log` subscriber that reentrantly calls `rand::rng()`, which AxiomVault itself does not install. **Risk depends on what the embedding application installs at runtime** (e.g. a mobile app with custom telemetry).

**Fix:** monitor RustSec for a fixed `rand` release; once available, run `cargo update -p rand`. Until then, document the constraint that subscribers/loggers must not invoke `rand::rng()` reentrantly. Consider adding the advisory to `deny.toml` ignore list with an expiry date and tracking issue.

---

### M-2 — Desktop OAuth client uses `client_secret` (no PKCE)

**Severity:** MEDIUM. **File:** `core/storage/src/gdrive/auth.rs:91-109` (and the analogous Dropbox/OneDrive auth modules).

RFC 8252 (OAuth 2.0 for Native Apps) requires PKCE for public clients and discourages embedded `client_secret`s, because anyone with the binary can extract them. AxiomVault distributes the desktop/mobile binary, then expects users to provide `AXIOMVAULT_GOOGLE_CLIENT_ID/SECRET` via env. While that pushes the secret out of the binary, it *also* means the secret can be replayed by anyone who can read the env (e.g. another local user, a compromised process).

**Fix:** switch to PKCE (`code_challenge`/`code_verifier`) and drop the client secret. The `oauth2` crate supports `set_pkce_challenge`. This eliminates a class of credential-theft scenarios.

---

### M-3 — OAuth CSRF token verification is the caller's job

**Severity:** MEDIUM. **File:** `core/storage/src/gdrive/auth.rs:124-147`.

`AuthManager::authorization_url()` returns `(url, csrf_token)`, but `exchange_code(code)` does **not** take the original token to verify against the callback's `state` parameter. CLI does verify (`tools/cli/src/main.rs:1312`), but other callers (mobile clients, future integrations) could forget — and the library wouldn't notice.

**Fix:** either (a) require the CSRF token as an argument to `exchange_code` and verify internally before exchanging, or (b) provide a higher-level method that takes the full callback URL and verifies state before exchanging code. Make the *unsafe* path (no verification) explicitly opt-in.

---

### M-4 — `LocalProvider` writes vault config and ciphertext with default umask

**Severity:** MEDIUM. **File:** `core/storage/src/local.rs:82-116`.

`fs::write` and `fs::rename` use the process umask, so on Linux the vault config (containing wrapped master keys + KDF params) and all ciphertext files are typically `0644` — readable by every local user. While the data is encrypted, this widens the attack surface unnecessarily: an attacker on the same host can offline-attack passwords against the wrapped key without escalating privileges.

**Fix:** explicitly `chmod 0600` (file) / `0700` (directory) after creation, or use `OpenOptions::mode(0o600)` on the temp path before rename.

---

### M-5 — Staging area files inherit default umask, may contain plaintext

**Severity:** MEDIUM. **File:** `core/sync/src/staging.rs:78-103`.

`StagingArea::stage_upload` calls `fs::write(&staging_file, &data)` with no permission bits set. Whether the data is plaintext depends on the caller; for any caller that stages plaintext (the vault's intent is to encrypt before staging, but that's a layering contract not enforced by the type), default umask leaves it world-readable.

**Fix:** set `0600` on the staging file. Additionally, document the layering contract loud in `staging.rs`: "The bytes passed here MUST be ciphertext."

---

### M-6 — `generate_conflict_path` uses second-resolution timestamps

**Severity:** MEDIUM. **File:** `core/sync/src/conflict.rs:118-130`.

`format!("%Y%m%d_%H%M%S")` — two conflicts on the same path within the same wall-clock second produce identical renamed paths. With `KeepBoth` strategy this means the second conflict's `provider.upload(&renamed_path, local_data)` silently overwrites the first conflict's file, losing data.

**Fix:** include sub-second precision (`%f` in chrono) and/or a short random suffix.

---

### M-7 — `EncryptingStream::encrypt_stream` buffers the entire encrypted output

**Severity:** MEDIUM. **File:** `core/crypto/src/stream.rs:77-115`.

The doc comment acknowledges the issue:

> The current implementation reads all encrypted chunks into a Vec before writing, because total_chunks is written in the header and cannot be known until all chunks are processed. For large files this doubles peak memory usage.

For a 4 GB file this means ~8 GB of resident memory. That is a DoS condition on phones / FUSE mounts and a non-trivial wallclock+I/O cost on desktops.

**Fix (recommended in the doc):** drop `total_chunks` from the header; rely on EOF. Alternatively, write to a tempfile and seek back to overwrite the header.

---

### M-8 — FUSE `open()` reads the full decrypted file into RAM

**Severity:** MEDIUM. **File:** `core/fuse/src/filesystem.rs:392-459`.

```rust
let buffer = match ops.read_file(&path).await { ... };  // entire plaintext in heap
```

The `OpenFile.buffer` lives until `release()`, so a malicious or buggy FUSE client opening many large files at once can exhaust memory. Plaintext exposure window is also longer than necessary.

**Fix:** chunked/streaming reads, or at minimum a configurable max-in-memory size. Pair with `EncryptingStream` once M-7 is fixed.

---

### M-9 — Missing `// SAFETY:` comments on `unsafe` blocks

**Severity:** MEDIUM. **File:** `core/fuse/src/filesystem.rs:42-43`.

```rust
uid: unsafe { libc::getuid() },
gid: unsafe { libc::getgid() },
```

Per `CLAUDE.md` and the pre-commit hook, every `unsafe` block requires a `// SAFETY:` comment. These are missing. The pre-commit hook should be catching this — verify it actually does (and if not, fix the hook).

**Fix:** add comments such as `// SAFETY: getuid/getgid are async-signal-safe POSIX syscalls and have no preconditions.` Then verify `.githooks/` actually rejects future violations.

---

### M-10 — Migration framework is local-disk only

**Severity:** MEDIUM. **File:** `core/vault/src/migration.rs`.

`MigrationRegistry::migrate` uses `std::fs::copy/read/write` directly on a `vault_path: &Path`. For cloud-backed vaults (Google Drive, Dropbox, etc.), this path doesn't exist — migration would fail. The actual v1.0→v1.1 upgrade logic lives in `VaultConfig::migrate_to_v1_1` (`config.rs:347`) and operates on the in-memory config; the registry's `MigrationV1_0ToV1_1` is a no-op placeholder that just bumps the version field.

**Risk:** future structural migrations can't be applied to cloud vaults without rework. Today this is dormant (only one migration exists, and it's a placeholder).

**Fix:** replace `vault_path: &Path` with `provider: &dyn StorageProvider` so backup/restore go through the storage abstraction.

---

### L-1 — `encrypt_with_nonce` is a public footgun

**File:** `core/crypto/src/aead.rs:104-135`. Public API; nonce reuse with the same key catastrophically breaks XChaCha20. The doc warns, but there is no programmatic safeguard (e.g., a typed wrapper that tracks (key, nonce) pairs, or `pub(crate)` visibility).

**Fix:** at minimum, restrict to `pub(crate)` if no external consumer needs it. If filename encryption needs determinism, build a typed `DeterministicCipher` newtype that owns its own nonce-derivation strategy.

### L-2 — `Salt` exposes inner bytes mutably

**File:** `core/crypto/src/keys.rs:152` — `pub struct Salt(pub [u8; 32])`. Salt is not secret, but the `pub` field invites bugs (e.g., overwriting after derivation). Use a private field with a getter.

### L-3 — `MasterKey::from_bytes(key: [u8; KEY_LENGTH])` takes the array by value

**File:** `core/crypto/src/keys.rs:33`. The caller's stack copy is not zeroized. Today most callers wrap their input in `Zeroizing<[u8; 32]>` and dereference; that drops the wrapper after the move, so the move itself is fine. But the type signature doesn't enforce this discipline. Consider taking `Zeroizing<[u8; 32]>` directly, or `&mut [u8; 32]` and zeroing in-place.

### L-4 — `.unwrap()` in production code path

**File:** `core/crypto/src/stream.rs:196` — `decrypted[..8].try_into().unwrap()`. The preceding length check makes it provably unreachable, but `.expect("len >= 8 verified above")` is more honest and would survive a future refactor that drops the check.

### L-5 — HTTP client lacks total request timeout

**File:** `core/storage/src/http_client.rs:25-31`. `connect_timeout(10s)` only covers TCP connect. A slow/malicious server can hang the whole sync forever. Add per-request timeouts, especially for metadata calls (which should be sub-second), while keeping streaming uploads/downloads unbounded.

### L-6 — Erasure-shard CRC-32 is non-cryptographic

**File:** `core/storage/src/composite/erasure.rs:144-149`. CRC-32 detects bit rot, not tampering. A backend that mutates a shard can recompute the CRC. This is fine in practice because the AEAD tag at the vault layer catches all tampering, but it's worth a comment that the CRC is *only* for integrity-against-noise, not authenticity.

### L-7 — Staging registry corruption is silent

**File:** `core/sync/src/staging.rs:65` — `serde_json::from_str(&content).unwrap_or_default()`. A corrupted registry silently drops all in-flight changes. Log a warning at minimum; consider failing fast so the user can recover.

### L-8 — Retry executor doesn't retry on `Authentication` errors

**File:** `core/sync/src/retry.rs:161-163`. Only `Network` and `Io` are retried. An expired token surfaces as `Authentication`, so the retry doesn't kick in to allow the token manager to refresh. In practice the `CloudTokenManager` refreshes proactively (5-min buffer), so this is rarely visible — but add `Authentication` to the retryable set if you ever miss the buffer window.

### L-9 — FUSE `open()` ignores access flags

**File:** `core/fuse/src/filesystem.rs:392`. `O_RDONLY`/`O_WRONLY`/`O_RDWR` are not honored — every open returns a writable handle. Kernel `default_permissions` mitigates this for cross-user access, but in-process logic that expects RDONLY semantics is unprotected.

### L-10 — CSRF token comparison is non-constant-time (CLI)

**File:** `tools/cli/src/main.rs:1312`. `received_state != csrf_token` is `String` `PartialEq`, which short-circuits on the first byte mismatch. CSRF tokens are short-lived single-use values delivered via HTTP, so a remote timing attack is impractical — but `subtle::ConstantTimeEq` is one line and removes the question.

---

## Informational notes

- **I-1:** `KdfParams::interactive` (64 MiB / 3 iter / 4 par) meets but does not exceed OWASP 2023 minimum (19 MiB / 2 iter / 1 par). For desktops, consider `sensitive` (256 MiB) as the default — Argon2id memory cost is the main defense against ASIC/GPU attackers.
- **I-2:** `MigrationRegistry::with_defaults()` registers `MigrationV1_0ToV1_1`, which is a no-op (just bumps version). The real v1.0→v1.1 wrapping logic is `VaultConfig::migrate_to_v1_1`. Two separate flows for the same conceptual change is confusing — pick one and document the relationship.

---

## Positive findings (defense-in-depth working as intended)

- AEAD primitive choice (XChaCha20-Poly1305) is correct: 24-byte random nonces are safe.
- Argon2id used with sane defaults; constant-time password verification via `subtle::ct_eq`.
- Master-key wrapping under both password-KEK and recovery-KEK — password change re-wraps the master key without re-encrypting all data.
- `Zeroize`/`ZeroizeOnDrop` consistently derived on `MasterKey`, `FileKey`, `DirectoryKey`, `RecoveryKey`, `CloudTokens`.
- `Debug` for key types is overridden to `[REDACTED]`.
- Stream-encryption per-chunk index is authenticated, defending against chunk reorder/injection.
- `VaultPath` rejects `.`, `..`, and path separators inside components — path traversal is blocked at the type level (verified by proptest in `core/common/src/types.rs`).
- `LocalProvider::upload` is atomic (write-temp-then-rename) — no torn writes.
- `ShardMap` uses tombstones to prevent resurrection of deleted entries when merging diverged backends.
- Pre-commit pipeline includes `gitleaks`, `cargo fmt --check`, `cargo clippy -D warnings`, and an unsafe-block SAFETY check.
- `cargo deny` is configured strictly: only crates.io sources, denylist for unknown registries/git.

---

## Recommended next steps (prioritized)

1. **`cargo update -p rustls-webpki`** to resolve C-1, then re-run `cargo audit`.
2. Fix H-1 (sync data loss) — this is silently corrupting state right now.
3. Pipe `Zeroizing` end-to-end through the FFI boundary (H-2, H-3).
4. Add `0600` mode to `LocalProvider` and staging writes (M-4, M-5).
5. Switch desktop OAuth to PKCE (M-2) and verify CSRF inside the library (M-3).
6. Add the missing `// SAFETY:` comments and verify the pre-commit hook actually rejects them in the future (M-9).
7. Track the `rand` advisory; add to `deny.toml` ignore with an expiry tracking issue if a fix is not yet released (M-1).

---

## Tooling provenance

- `cargo audit` ran against advisory-db `1049 advisories` snapshot.
- `cargo deny check` — `advisories FAILED, bans ok, licenses ok, sources ok` (the FAILED line is C-1 / M-1).
- Plugin: `anthropic-cybersecurity-skills` (commit `72a2cd6`, 754 skills).
- No dynamic analysis (fuzzing, sanitizer runs) was performed — recommended as a follow-up for `core/crypto` and `core/storage/composite/erasure`.
