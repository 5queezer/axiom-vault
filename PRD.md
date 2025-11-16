AxiomVault

Product Requirements Document

1. Objective
   Deliver a cross-platform encrypted-vault system mirroring the functional envelope of Cryptomator while remaining implementation-original. Provide a Rust core for encryption, vault structure, and abstracted storage. Provide mobile and desktop clients. Provide initial Google Drive integration with a pluggable storage factory enabling additional providers such as iCloud without architectural revision.

2. Scope
   Core vault engine. Storage provider abstraction. Google Drive provider. iOS client. Android client. Linux and macOS desktop client. FUSE mounting. Vault creation, unlocking, file operations, sync, conflict handling, and background tasks.

3. Out-of-Scope
   Windows client in first release. Multi-user sharing. Server-side coordination. Real-time collaborative features. Web client.

4. Users
   Single user storing personal files across devices seeking client-side encryption independent of cloud provider trust.

5. Functional Requirements

5.1 Vault Lifecycle
Create vault with password.
Derive master key using Argon2id.
Generate directory and file obfuscation keys.
Persist vault metadata in encrypted form.
Unlock vault producing a session handle.
Relock vault clearing all keys from memory.

5.2 Cryptography
Encrypt files using XChaCha20-Poly1305 streaming.
Encrypt filenames deterministically with per-file nonce derivation.
Authenticate directory structures.
Zero memory for keys on drop.
Use RustCrypto or orion primitives exclusively.
Support rekeying for password rotation.

5.3 Virtual Filesystem
Represent vault structure in an abstract tree independent of provider.
Support file create, read, write, delete.
Support directory create, list, delete.
Provide unified error semantics across storage backends.
Enable conflict detection based on etags or revision IDs.
Provide resolution: keep-both, prefer-local, prefer-remote.

5.4 Storage Provider Interface
Trait: `StorageProvider`.
Operations: upload(stream), download(stream), exists, delete, list, metadata, create_dir, delete_dir.
Async trait.
Provider factory: register by name, resolve by configuration.
Google Drive provider: OAuth2, tokens, folder IDs, revision tracking, chunked uploads, resumable uploads.
Provider isolation: no provider-specific logic allowed in vault or crypto modules.

5.5 Sync Engine
Two modes: on-demand (file operations trigger remote operations) and periodic.
Local staging area for writes.
Atomic write model: temp object → commit.
Conflict detection.
Retry strategy for transient network errors.
Exponential backoff.
Background tasks for mobile platforms.

5.6 iOS Client
SwiftUI interface.
Rust core compiled as static library.
C-ABI through cbindgen.
File provider extension optional for mounting.
Authentication UI for Google Drive via ASWebAuthenticationSession.
Local cached index of vault.
Background sync via BGTaskScheduler.
Biometric unlock optional at UI level but never bypassing core password.

5.7 Android Client
Kotlin Compose interface.
Rust core compiled as shared library via JNI.
Google Drive authentication via OAuth WebView + redirect.
WorkManager for periodic sync.
Secure storage of tokens in Android Keystore.

5.8 Linux/Desktop
Use Tauri or native Rust GUI.
FUSE mount on Linux.
macFUSE on macOS.
Local index persisted to SQLite or simple binary storage.
Direct Rust linkage; no FFI.

5.9 Configuration and Metadata
Config stored at vault root in encrypted config file.
Includes provider type, provider-specific config, KDF parameters, versioning.
Support migrations for future vault format versions.

5.10 Logging
Use `tracing`.
Structured logs.
Sensitive data never logged.
Per-provider request IDs for debugging.

6. Non-Functional Requirements

6.1 Security
Zeroize key material.
No plaintext path or content leaves core.
No dependency that performs logging of cryptographic state.
Constant-time comparison for sensitive checks.
Separation of authentication tokens from vault logic.

6.2 Performance
Streaming encryption for large files.
Parallel chunk uploads where supported by provider.
Index diff optimized to reduce remote calls.
Minimal memory footprint on mobile.

6.3 Portability
Rust core must compile to iOS, Android (ARM and x86), Linux, and macOS without conditional logic except platform FFI.
No platform-specific crypto dependencies.

6.4 Modularity
New providers added by writing a module implementing `StorageProvider` and registering the factory element.
No cross-module import from provider into vault or crypto.

6.5 Testability
Mock storage provider for unit tests.
Property-based tests for encryption invariants.
Load tests for large directory trees.
FUSE integration tests on CI for Linux.

7. Architecture

7.1 Repository Structure

```
/core
    /crypto
    /vault
    /storage
        /gdrive
        /providers          (registry)
    /ffi
        ios/
        android/

/clients
    /ios
    /android
    /desktop

/tools
    cli/

/docs
```

7.2 Modules
Crypto: key derivation, AEAD, streaming encryption.
Vault: metadata, tree representation, conflict logic.
Storage: provider trait, provider registry, Google Drive implementation.
FFI: C interface generation, Swift and JNI bindings.
Clients: UI, platform integration.
CLI: debug and testing tool.

7.3 Data Flow
UI → FFI (mobile) or Rust direct (desktop) → Vault engine → Storage provider → Remote cloud.

8. Milestones

M1: Core vault engine, crypto, CLI tool.
M2: Storage provider factory, Google Drive provider.
M3: Sync engine.
M4: iOS bindings and UI.
M5: Android bindings and UI.
M6: Desktop client and FUSE integration.
M7: Security audit.
M8: Packaging and deployment.

9. Acceptance Criteria
   Vault creation, unlocking, and FUSE mount functional on Linux and macOS.
   iOS and Android apps can create vault, upload, download, edit files.
   Google Drive sync stable under weak networks.
   No plaintext leakage verified by tests.
   Provider extensibility validated by compiling a mock provider.

Coding Standards

1. Core Principles
   Single-responsibility modules.
   Immutable data where possible.
   Explicit ownership boundaries.
   Minimal surface area for public APIs.
   Eliminate hidden side effects.
   Avoid temporal coupling.
   Stability-first refactoring schedule to keep churn low.

2. Rust Core Practices
   No panics in library code.
   Use `Result<T, E>` everywhere.
   Prefer narrow traits with clear contracts.
   Enforce invariants via type system: newtypes for keys, paths, identifiers.
   Limit unsafe to audited, isolated blocks.
   Centralize error types with `thiserror`.
   Use `#[automatically_derived]` for predictable trait impls only where semantically valid.
   Use exhaustive pattern matches.
   Forbid wildcard matches in sensitive code.
   Apply `clippy` with all pedantic lints.
   Enforce `rustfmt` with project-wide configuration.

3. Modularity and Reuse
   Crypto module has no dependency on storage or vault structures.
   Vault module depends only on crypto and abstract storage.
   Storage providers depend only on abstract trait definitions.
   FFI layer depends only on stable core APIs.
   Clients depend exclusively on FFI or direct Rust interface.
   Internal helpers (streams, buffers, async utilities) placed in a `/core/common` module for reuse.
   No cyclic dependencies.
   Provider additions never require touching core modules.

4. Interface Stability
   Public Rust core API frozen once mobile bindings are generated.
   Changes to FFI functions versioned explicitly.
   Stabilize trait definitions early to reduce provider churn.
   Maintain backward-compatible vault format versions.

5. Minimal Churn Guidelines
   Plan structural decisions before implementation.
   Avoid renaming public symbols after exposure to mobile or desktop layers.
   Document intent and invariants in `architecture.md` to eliminate speculative refactoring.
   Enforce “no mechanical refactors” unless justified through performance or security.
   Batch non-critical changes into periodic maintenance windows.

6. Code Clarity
   No embedded business logic in UI layers.
   No premature abstractions: implement minimal viable trait sets.
   Remove obsolete feature flags immediately after consolidation.
   Prefer explicit builders over multi-parameter constructors.
   Use descriptive identifiers for encrypted structures rather than generic names.

7. Testing Discipline
   Unit tests follow arrange–act–assert with no hidden state.
   Crypto tests use known test vectors.
   Storage tests fully mocked to isolate provider failures.
   FS logic tests use temporary directories with deterministic data.
   Concurrency tests validate ordering guarantees explicitly.

8. Documentation
   Every public function documented with preconditions, postconditions, and failure paths.
   State diagrams for vault sessions.
   Sequence diagrams for upload and download operations.
   FFI function documentation mirrors Rust comments in generated headers.

9. Performance Without Obfuscation
   Optimize only after measuring.
   Document rationale for major optimizations such as custom buffer pools.
   Maintain readable streaming pipelines.
   Avoid speculative caching and premature data structures.

10. Code Review Rules
    Reject changes introducing cross-module coupling.
    Reject changes weakening type safety.
    Reject unscoped feature additions.
    Reject patches that modify formatting or naming without functional need.
    Require explicit reasoning for all structural changes.


