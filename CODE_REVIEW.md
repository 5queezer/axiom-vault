# AxiomVault Code Review

**Date**: 2026-03-01
**Scope**: Full codebase review covering all core modules, clients, CI/CD, and build configuration
**Commit**: 5ba1878 (HEAD of main)

---

## Executive Summary

AxiomVault is a well-architected cross-platform encrypted vault system with solid foundations in its cryptographic primitives and modular design. The code demonstrates strong Rust practices in the core crypto and vault modules. However, the review identified several issues across **security**, **correctness**, **concurrency**, and **code quality** that should be addressed before production use.

### Findings by Severity

| Severity | Count | Categories |
|----------|-------|------------|
| **Critical** | 4 | Credential exposure, query injection, constant-time comparison, FUSE mount non-functional |
| **High** | 15 | Race conditions, streaming defeats, sync concurrency, FFI memory leaks, disabled CSP, CDN supply-chain risk, data loss (MemoryProvider in prod) |
| **Medium** | 24 | Token storage, retry logic, incomplete implementations, TOCTOU races, path traversal, password zeroization |
| **Low** | 20 | Dead code, input validation, UI/UX gaps, minor ergonomic issues |

---

## 1. Cryptographic Module (`core/crypto/`)

### Strengths
- Correct use of XChaCha20-Poly1305 with 24-byte random nonces
- Argon2id with configurable strength levels (interactive, moderate, sensitive)
- All key types implement `Zeroize` and `ZeroizeOnDrop`
- Debug implementations redact key material
- Good test coverage including roundtrip, tamper detection, and edge cases

### Issues

#### CRITICAL: `verify_password` constant-time comparison is not actually constant-time
**File**: `core/crypto/src/kdf.rs:114-121`

```rust
let mut equal = true;
for (a, b) in derived.as_bytes().iter().zip(expected.as_bytes().iter()) {
    equal &= a == b;
}
```

The `==` operator on `u8` may short-circuit at the LLVM level due to optimization. The compiler can detect that `equal &= (a == b)` is equivalent to `equal = equal && (a == b)` and apply short-circuit evaluation. Use the `subtle` crate's `ConstantTimeEq` trait instead, or use `ct_eq` from the `chacha20poly1305` dependency tree.

#### MEDIUM: Streaming encryption reads all data into memory before encrypting
**File**: `core/crypto/src/stream.rs:64-96`

`EncryptingStream::encrypt_stream` reads all chunks into a `Vec` before writing any output. This defeats the purpose of streaming encryption for large files. The header contains `total_chunks` which requires knowing the count upfront, but this could be handled by writing a placeholder header and seeking back, or by using a format that doesn't require the count.

#### MEDIUM: Streaming decryption uses fixed buffer based on chunk_size from header
**File**: `core/crypto/src/stream.rs:149`

```rust
let encrypted_chunk_size = NONCE_SIZE + chunk_size + 8 + TAG_SIZE;
```

A maliciously crafted header with a very large `chunk_size` (e.g., `u32::MAX`) would cause allocation of a ~4GB buffer. There should be a maximum allowed chunk size validation.

#### LOW: `read_chunk` has fragile end-of-chunk detection
**File**: `core/crypto/src/stream.rs:182-206`

The `read_chunk` function uses a heuristic to detect chunk boundaries by checking "if we've read enough." This can fail if the underlying reader returns partial reads, potentially splitting or merging chunks incorrectly.

---

## 2. Vault Module (`core/vault/`)

### Strengths
- Clean session lifecycle management with `lock()`/`unlock()`/`drop()` semantics
- Password verification using encrypted constant (prevents key exposure)
- Virtual tree properly separates logical structure from storage
- Version compatibility checking for migration support

### Issues

#### MEDIUM: `VaultSession::unlock` derives the master key twice
**File**: `core/vault/src/session.rs:82-99`

`config.verify_password(password)` derives the key internally, then `derive_key()` is called again to produce the master key. For Argon2id, key derivation is intentionally expensive (0.5-1s). This doubles the unlock time unnecessarily. Instead, `verify_password` should return the derived key on success.

#### MEDIUM: `change_password` does not re-encrypt existing files
**File**: `core/vault/src/session.rs:191-214`

When the password is changed, a new salt and verification data are generated, but file keys are derived from the master key, which changes. This means all existing encrypted files become undecryptable after a password change because their file keys will derive differently from the new master key.

#### MEDIUM: Filename encryption is non-deterministic but used for lookup
**File**: `core/vault/src/operations.rs:28-33`

`encrypt_name` uses `encrypt()` which generates a random nonce each time. The encrypted name is stored in the tree and used as the storage path. However, the same filename encrypted twice will produce different encrypted names. This is correct for security but means filenames cannot be looked up by re-encrypting — they must be found via the tree. This is the current design but worth noting explicitly.

#### LOW: `VaultTree` stores cleartext filenames
**File**: `core/vault/src/tree.rs:23`

The `NodeMetadata::name` field stores the original cleartext filename. When the tree is serialized to JSON and saved to storage (`m/tree.json`), cleartext filenames are exposed. The tree should be encrypted before persisting to storage.

---

## 3. Storage Module (`core/storage/`)

### Strengths
- Clean trait-based abstraction with `StorageProvider`
- Dynamic provider registry with factory pattern
- Good separation between local, memory, and cloud providers

### Issues

#### CRITICAL: Hardcoded OAuth2 credentials
**File**: `core/storage/src/gdrive/auth.rs:13-15`

```rust
const GOOGLE_CLIENT_ID: &str = "YOUR_CLIENT_ID";
const GOOGLE_CLIENT_SECRET: &str = "YOUR_CLIENT_SECRET";
```

Placeholder credentials are compiled into the binary via `AuthConfig::default()`. No runtime guard rejects these placeholders. If a developer ever substitutes real credentials, they become baked into version control history.

**Recommendation**: Require credentials via runtime configuration. Add validation in `AuthManager::new()` that rejects placeholder values.

#### CRITICAL: Google Drive API query injection
**File**: `core/storage/src/gdrive/client.rs:158-159, 197-200`

In `list_folder`, `folder_id` is inserted into the query string without sanitization:
```rust
let query = format!("'{}' in parents and trashed = false", folder_id);
```

In `find_file`, the `name` escaping is incomplete (only escapes `'` but not `\`), and `parent_id` is completely unsanitized. A crafted ID could inject additional query clauses.

**Recommendation**: Validate that IDs conform to expected format (alphanumeric) and use proper escaping for both quotes and backslashes.

#### HIGH: All `upload_stream`/`download_stream` implementations defeat streaming
**Files**: `local.rs`, `memory.rs`, `gdrive/provider.rs`

All three providers collect the entire stream into a `Vec<u8>` before uploading, completely negating the purpose of streaming and risking OOM on large files. The trait explicitly promises "streaming without loading entire file into memory."

#### HIGH: Non-deterministic metadata IDs in `LocalProvider`
**File**: `core/storage/src/local.rs:61`

Every call to `create_metadata` generates a new random UUID. Calling `metadata()` twice on the same file returns different IDs. Any code depending on stable IDs (caching, conflict detection) will break.

#### HIGH: TOCTOU races in `MemoryProvider` locking
**File**: `core/storage/src/memory.rs:215-280`

`create_dir` reads the parent under a read lock, drops it, then acquires a write lock to insert. Between the two lock acquisitions, another thread could delete the parent or create the same entry.

#### HIGH: Unbounded and incompletely invalidated path cache
**File**: `core/storage/src/gdrive/provider.rs:37, 171-175`

The GDrive `path_cache` grows without limit. When a directory is deleted/renamed, only the exact path is invalidated — child paths remain stale.

#### MEDIUM: Plaintext token storage
**Files**: `gdrive/auth.rs`, `gdrive/provider.rs`

OAuth2 tokens (`access_token`, `refresh_token`) are stored as plain strings. For a security-focused vault application, tokens should be encrypted at rest.

#### MEDIUM: No CSRF token verification for OAuth flow
**File**: `core/storage/src/gdrive/auth.rs:101-111`

The `authorization_url` method returns a CSRF token but no infrastructure exists to verify it during the callback.

#### MEDIUM: Static multipart boundary
**File**: `core/storage/src/gdrive/client.rs:239`

Using `"AxiomVaultBoundary"` as a static boundary. If file content contains this string, multipart parsing will break.

#### MEDIUM: `upload` takes `Vec<u8>` by value
**File**: `core/storage/src/provider.rs:67`

Accepting `Vec<u8>` forces callers to transfer ownership. `&[u8]` would be more flexible.

---

## 4. Sync Module (`core/sync/`)

### Strengths
- Two-mode sync (on-demand and periodic) with configurable scheduler
- Retry with exponential backoff and jitter
- Conflict resolution with multiple strategies

### Issues

#### HIGH: No concurrent sync guard — `sync_in_progress` flag is ineffective
**File**: `core/sync/src/engine.rs:157-203`

The flag is set inside a write lock block that is immediately dropped. A second concurrent `sync_full` call can read `sync_in_progress` as `false` in the window between the first call setting it and the second checking it. Two full syncs can run simultaneously, causing duplicate uploads and conflicting state mutations.

#### HIGH: Non-atomic check-and-upload in `upload_staged_file`
**File**: `core/sync/src/engine.rs:331-418`

The method reads sync state, drops the lock, makes a network call, then makes an upload decision based on the now-stale state. The race window spans a network round-trip.

#### HIGH: Blocking sync in scheduler `select!` loop
**File**: `core/sync/src/scheduler.rs:158-213`

While one sync is executing, the entire scheduler loop is blocked. No new requests can be received and periodic timers are stalled.

#### HIGH: Non-atomic staging operations
**File**: `core/sync/src/staging.rs:78-102`

Crash between file write and registry persist creates orphaned staging files. `persist_registry` uses non-atomic `fs::write`, risking corruption.

#### HIGH: False conflict detection
**File**: `core/sync/src/conflict.rs:88-109`

A locally-modified file with an unchanged remote is incorrectly flagged as a conflict. The fallthrough logic at line 108 returns `true` when both etags are `Some` and differ, regardless of whether the remote changed from its last known state.

#### MEDIUM: Retry returns second-to-last error
**File**: `core/sync/src/retry.rs:141-143`

On exhaustion, `last_error.unwrap_or(err)` returns the previous attempt's error (stored in `last_error`), not the most recent failure (`err`).

#### MEDIUM: Downloaded data is discarded
**File**: `core/sync/src/engine.rs:516`

`download_remote_changes` fetches file data but throws it away. The state is updated to `Synced` but the local copy is never actually updated.

#### MEDIUM: `rollback` is identical to `commit`
**File**: `core/sync/src/staging.rs:175-178`

Both operations do exactly the same thing (remove the staged change), losing any semantic distinction.

---

## 5. FFI Module (`core/ffi/`)

### Strengths
- Thread-local error storage is the correct pattern for FFI
- Consistent null pointer checks on all public functions
- Clean error code convention (0 success, -1 error)

### Issues

#### HIGH: `get_vault_info` leaks CString memory on error
**File**: `core/ffi/src/vault_ops.rs:131-137`

If the second `CString::new()` fails after the first has already been `.into_raw()`ed, the first string's memory is leaked.

#### HIGH: `block_on` will panic if called from an existing Tokio runtime
**File**: `core/ffi/src/lib.rs`

All FFI functions use `runtime.block_on(...)`. If called from within an async context, this causes a panic. There is no guard against misuse.

#### MEDIUM: Passwords not zeroized after use
**File**: `core/ffi/src/vault_ops.rs`

Passwords are passed as `&str` and converted to `&[u8]` but never zeroized. The `SensitiveBytes` type exists in common but is not used.

#### MEDIUM: No runtime shutdown mechanism
**File**: `core/ffi/src/runtime.rs`

Once initialized, the Tokio runtime lives forever. Mobile platforms (iOS/Android) may need explicit cleanup during app lifecycle events.

#### LOW: Dead code — `FFIFileEntry`, `FFISyncStatus`, `FFIConflictStrategy` defined but never used
**File**: `core/ffi/src/types.rs`

#### LOW: `VaultManager` wrapper struct in `vault_ops.rs` is defined but never used

---

## 6. FUSE Module (`core/fuse/`)

### Issues

#### HIGH: FUSE session is never started
**File**: `core/fuse/src/mount.rs:158`

`MountHandle._thread` is always set to `None`. The FUSE session is created but `session.run()` is never called. The mounted filesystem will not process any operations.

#### MEDIUM: `readdir` hardcodes parent inode to root
**File**: `core/fuse/src/filesystem.rs:350`

`let parent_ino = if ino == 1 { 1 } else { 1 };` — The `..` entry always points to inode 1. This is incorrect for nested directories.

#### MEDIUM: `setattr` silently ignores all attribute changes
**File**: `core/fuse/src/filesystem.rs:881-904`

`truncate()`, `chmod()`, `chown()`, `utimensat()` all appear to succeed but do nothing.

#### MEDIUM: `open` reads entire file into memory
**File**: `core/fuse/src/filesystem.rs:423`

For large encrypted files, this means the entire decrypted content sits in memory.

---

## 7. Common Module (`core/common/`)

### Issues

#### MEDIUM: `VaultPath::parse` does not reject `.` or `..` components
**File**: `core/common/src/types.rs:88-100`

Path traversal components are accepted, which could lead to security issues if vault paths are ever mapped to real filesystem paths.

#### LOW: Duplicate error variants `NotPermitted` and `PermissionDenied`
**File**: `core/common/src/error.rs:33-34, 57-58`

Both exist with unclear semantic distinction. Consider consolidating.

#### LOW: `VaultId::new` does not reject null bytes, control characters, or path separators
**File**: `core/common/src/types.rs:22-29`

---

## 8. CLI (`tools/cli/`)

### Strengths
- Clean `clap`-based interface with well-documented subcommands
- Comprehensive command set covering vault lifecycle, file operations, sync, and Google Drive
- CSRF token validation in OAuth flow
- Consistent error handling with `anyhow::Context`

### Issues

#### MEDIUM: Passwords not zeroized from memory
**File**: `tools/cli/src/main.rs:324-327`

The `String` from `rpassword::prompt_password` is converted to `Vec<u8>` but neither is zeroized. The password lingers in heap memory.

#### MEDIUM: OAuth tokens saved with world-readable permissions
**File**: `tools/cli/src/main.rs:785-787`

`tokio::fs::write` uses umask-dependent mode (typically 0644). Token files should be created with 0600.

#### MEDIUM: OAuth callback has no timeout and accepts any first connection
**File**: `tools/cli/src/main.rs:699-702`

The TCP listener binds to port 8080 and waits indefinitely for one connection. A non-OAuth request (port scanner, browser preflight) hitting it first will consume the callback.

#### LOW: Client secret passable via CLI arguments
**File**: `tools/cli/src/main.rs:138-143`

CLI arguments are visible in `/proc/<pid>/cmdline` and shell history.

#### LOW: Significant code duplication across vault subcommands

Every command that operates on an existing vault repeats the same ~10 lines for password prompting, provider config, manager creation, and `open_vault`. This should be extracted into a shared helper.

---

## 9. Desktop Client (`clients/desktop/`)

### Strengths
- Clean Tauri 2.0 architecture with separate state management
- SQLite-based local index for fast metadata queries
- Well-structured Vue 3 Composition API frontend

### Issues

#### HIGH: Content Security Policy disabled
**File**: `clients/desktop/src-tauri/tauri.conf.json:25`

```json
"security": { "csp": null }
```

CSP is set to `null`, meaning no protection against injected scripts. Any XSS in the webview gains full access to the Tauri IPC bridge.

#### HIGH: Vue loaded from remote CDN at runtime
**File**: `clients/desktop/src/app.js:38`

```javascript
const { createApp, ref, computed, onMounted } = await import('https://unpkg.com/vue@3/dist/vue.esm-browser.js');
```

No SRI hash, no version pinning. If unpkg.com is compromised, the attacker gets full JS execution with access to all IPC commands. Combined with the disabled CSP, this is the most critical client-side issue.

#### HIGH: MemoryProvider always used — all vault data lost on app close
**File**: `clients/desktop/src-tauri/src/commands.rs:62-67`

```rust
// For now, use memory provider for testing
let provider = Arc::new(MemoryProvider::new());
```

This is labeled "for testing" but is in production code. All vaults use in-memory storage and data is lost when the application closes.

#### MEDIUM: Mount point path from frontend is unsanitized
**File**: `clients/desktop/src-tauri/src/commands.rs:179-181`

The frontend can request mounting at any arbitrary filesystem path. The backend will create the directory tree without validation.

#### MEDIUM: Vault ID used in filesystem paths without sanitization
**File**: `clients/desktop/src-tauri/src/commands.rs:73`

User-provided vault ID is used directly in `format!("{}.db", id)` for the SQLite path. Path traversal via `../../` is possible.

#### MEDIUM: XSS in error initialization screen
**File**: `clients/desktop/src/app.js:19`

Error message interpolated directly into `innerHTML` without escaping.

#### MEDIUM: `update_file` does not update the local index
**File**: `clients/desktop/src-tauri/src/commands.rs:319-339`

After updating a file, the local index retains stale size and modification time.

#### MEDIUM: `lock_vault` does not unmount FUSE first
**File**: `clients/desktop/src-tauri/src/commands.rs:157-168`

If a vault is mounted and locked, the FUSE mount is left in a stale state.

#### MEDIUM: All errors flattened to `String`
Throughout `commands.rs`, every error is `.to_string()`ed, discarding type and cause chain. The frontend cannot distinguish error types.

#### LOW: SQLite local index stores plaintext metadata
**File**: `clients/desktop/src-tauri/src/local_index.rs:36`

File paths, sizes, and timestamps stored unencrypted, partially defeating vault encryption.

---

## 10. CI/CD and Build Configuration

### Strengths
- Comprehensive CI pipeline: fmt, clippy, test, individual crate builds, security audit, doc build, FFI header generation
- Multi-platform testing (Ubuntu + macOS, stable + MSRV)
- Good PR workflow with size checks, metadata validation, and path-based filtering
- Release pipeline with multi-arch CLI builds, iOS XCFramework, checksums

### Issues

#### MEDIUM: Security audit uses `continue-on-error: true`
**File**: `.github/workflows/rust-ci.yml:183`

Security vulnerabilities will be reported but will never fail CI. This should at minimum be a required check for releases.

#### MEDIUM: `Cargo.lock` is not committed
The `.gitignore` excludes `Cargo.lock`. For an application/binary project, committing the lock file ensures reproducible builds. The CI workflows reference `${{ hashFiles('**/Cargo.lock') }}` for cache keys, which will always be empty.

#### LOW: PR title check uses unsanitized input
**File**: `.github/workflows/pr-check.yml:55-64`

`TITLE="${{ github.event.pull_request.title }}"` — If the PR title contains shell metacharacters, this could cause unexpected behavior. Use environment variable instead of direct interpolation.

#### LOW: Release workflow uses deprecated `actions/create-release@v1`
**File**: `.github/workflows/release.yml:49`

This action is archived and no longer maintained. Consider using `softprops/action-gh-release` or the `gh` CLI.

---

## 11. Architectural Recommendations

### Priority 1: Security Fixes
1. Replace the hand-rolled constant-time comparison with `subtle::ConstantTimeEq`
2. Remove hardcoded OAuth2 credentials; require runtime configuration
3. Sanitize Google Drive API query parameters
4. Encrypt the vault tree before persisting (cleartext filenames are exposed)

### Priority 2: Correctness Fixes
1. Fix `change_password` to re-encrypt file keys or decouple file keys from the master key
2. Fix false conflict detection in sync module
3. Fix the FUSE mount to actually start the session
4. Add a concurrent sync guard (e.g., `tokio::sync::Mutex` or `Semaphore`)
5. Make staging operations atomic (write-to-temp + rename)

### Priority 3: Performance & Robustness
1. Implement actual streaming for `upload_stream`/`download_stream` (at least for `LocalProvider`)
2. Avoid double key derivation in `VaultSession::unlock`
3. Add maximum chunk size validation in streaming decryption
4. Bound the GDrive path cache size and fix child invalidation
5. Fix retry to return the most recent error

### Priority 4: Code Quality
1. Remove dead code (`FFIFileEntry`, `FFISyncStatus`, `WithRetry` trait, etc.)
2. Commit `Cargo.lock` for reproducible builds
3. Make security audit a required CI check
4. Add structured logging across storage providers
5. Add path traversal validation (reject `.` and `..` components)

---

## Test Coverage Assessment

The codebase has unit tests for all core modules with reasonable coverage:
- **Crypto**: Roundtrip, tamper detection, empty input, large input, nonce uniqueness
- **Vault**: Session lifecycle, password verification, tree operations, file CRUD
- **Storage**: Provider registry, metadata serialization
- **Sync**: State transitions, conflict detection, retry logic, staging persistence

**Notable gaps**:
- No integration tests across module boundaries
- No property-based tests (proptest is in dependencies but unused)
- No tests for concurrent operations (race conditions)
- No tests for error recovery scenarios
- FUSE module has no tests at all
