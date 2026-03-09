# Native Desktop Architecture

This document defines the **target architecture** for AxiomVault's native desktop clients. It establishes the boundary between shared Rust core logic and platform-native UI shells, guiding implementation so that product behavior stays centralized while presentation stays native.

> **Status:** This is a design document describing the intended end state. Sections marked *(existing)* reflect what is already implemented; everything else is proposed and subject to refinement during implementation.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                   Platform UI Shell                      │
│  ┌──────────────────┐  ┌──────────────────────────────┐ │
│  │  macOS (SwiftUI)  │  │  Linux (GTK4 / libadwaita)  │ │
│  └────────┬─────────┘  └─────────────┬────────────────┘ │
│           │ Swift FFI (C-ABI)        │ Direct Rust link  │
├───────────┴──────────────────────────┴──────────────────┤
│                   Application Facade                     │
│              (core/app — shared Rust crate)              │
│                                                          │
│  ┌──────────┐ ┌──────────┐ ┌───────┐ ┌──────────────┐  │
│  │  Vault   │ │ Storage  │ │ Sync  │ │    Events    │  │
│  │  Engine  │ │Providers │ │Engine │ │   (channels) │  │
│  └────┬─────┘ └────┬─────┘ └───┬───┘ └──────┬───────┘  │
│       │            │           │             │           │
│  ┌────┴────────────┴───────────┴─────────────┘          │
│  │                 Crypto Core                           │
│  │  XChaCha20-Poly1305 · Argon2id · Blake2b · Zeroize  │
│  └──────────────────────────────────────────────────────┘│
│                   Common Types                           │
│            VaultId · VaultPath · Error                    │
└─────────────────────────────────────────────────────────┘
```

## Design Principles

1. **One product brain, multiple native shells.** All product logic lives in Rust. UI shells are thin wrappers over a shared application facade.

2. **UI owns presentation, not product logic.** Clients render state and dispatch user intents. They never encrypt, decrypt, resolve conflicts, manage sessions, or talk to storage providers directly.

3. **Shared contracts, not conventions.** Clients consume the same Rust API surface through typed DTOs and a defined error taxonomy, ensuring behavioral convergence without manual coordination.

4. **Event-driven state flow.** The core emits domain events (vault opened, file changed, sync conflict, etc.) over a typed channel. Clients subscribe and update their UI reactively.

5. **Platform integration is a client concern.** Keychain access, biometric prompts, file pickers, system notifications, tray/menu bar integration, and desktop environment theming belong in the UI shell.

## Layer Boundaries

### Shared Rust Core (owns product logic)

| Layer | Crate | Responsibility |
|-------|-------|---------------|
| **Common** | `axiomvault-common` *(existing)* | `VaultId`, `VaultPath`, `Error`, shared types |
| **Crypto** | `axiomvault-crypto` *(existing)* | AEAD, KDF, key types, streaming encryption, recovery keys |
| **Vault** | `axiomvault-vault` *(existing)* | `VaultSession`, `VaultConfig`, `VaultTree`, `VaultOperations`, `VaultManager` |
| **Storage** | `axiomvault-storage` *(existing)* | `StorageProvider` trait, Local/GDrive/Dropbox/OneDrive/iCloud providers, `ProviderRegistry` |
| **Sync** | `axiomvault-sync` *(existing)* | `SyncEngine`, `SyncScheduler`, `ConflictResolver`, `StagingArea`, retries |
| **FUSE** | `axiomvault-fuse` *(existing)* | Filesystem mounting (Linux via libfuse3, macOS via macFUSE) |
| **App Facade** | `axiomvault-app` *(proposed)* | Stateful application API, session management, event emission, DTO layer |

### Application Facade (proposed crate: `axiomvault-app`)

The facade is the single entry point for all desktop clients. It wraps the lower-level vault, storage, and sync crates into a coherent application API.

**Responsibilities:**
- Vault lifecycle: create, open, lock, unlock, close
- File operations: list, read, write, create, delete, move
- Session state: active vault, current user, lock status
- Password management: change password, verify recovery key
- Sync orchestration: trigger sync, report progress, handle conflicts
- Event emission: broadcast domain events to subscribers
- Health checks: vault integrity verification
- Provider management: configure and switch storage backends

**API style:** The facade exposes `async` methods. How the async boundary is presented to each platform is a bridge/adapter concern:

- **Linux (GTK4):** The Rust binary owns the Tokio runtime and can call `async` methods directly.
- **macOS (Swift FFI):** The FFI bridge can block-on-runtime, spawn-and-callback, or expose a polling model — whichever best suits cancellation, progress streaming, and UI responsiveness. The facade does not prescribe a specific strategy.

```rust
// Conceptual API sketch (async — bridge adapters wrap as needed)
pub struct AppService { /* ... */ }

impl AppService {
    pub fn new() -> Self;

    // Vault lifecycle
    pub async fn create_vault(&self, params: CreateVaultParams) -> Result<VaultCreatedDto>;
    pub async fn open_vault(&self, params: OpenVaultParams) -> Result<VaultInfoDto>;
    pub async fn lock_vault(&self) -> Result<()>;
    pub async fn close_vault(&self) -> Result<()>;

    // File operations
    pub async fn list_directory(&self, path: &str) -> Result<Vec<DirectoryEntryDto>>;
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>>;
    pub async fn create_file(&self, path: &str, data: &[u8]) -> Result<()>;
    pub async fn update_file(&self, path: &str, data: &[u8]) -> Result<()>;
    pub async fn delete_file(&self, path: &str) -> Result<()>;
    pub async fn create_directory(&self, path: &str) -> Result<()>;

    // Events
    pub fn subscribe(&self) -> EventReceiver;

    // Health & info
    pub async fn vault_info(&self) -> Result<VaultInfoDto>;
}
```

### Platform UI Shell (owns presentation) *(proposed)*

Each UI shell is a thin native application that:

1. Initializes the `AppService`
2. Subscribes to events
3. Translates user interactions into facade method calls
4. Renders state from DTOs and events
5. Handles platform-specific integrations

**macOS shell (SwiftUI/AppKit):** *(proposed — refines existing iOS FFI pattern)*
- Bridged via C-ABI FFI (same pattern as current iOS client)
- Existing `VaultCore.swift` + `VaultManager.swift` adapted to call facade
- Keychain integration for biometric unlock
- Menu bar / status bar item
- NSFileCoordinator for Finder integration
- Drag-and-drop, Quick Look, Spotlight metadata

**Linux shell (GTK4/libadwaita):** *(proposed)*
- Direct Rust linkage (no FFI overhead)
- GTK4 + libadwaita for GNOME HIG compliance
- Secret Service API for keyring integration
- Desktop notifications via libnotify
- File manager integration via D-Bus / portal APIs
- XDG conventions for data/config/cache directories

### Do Not Implement in UI

The following must **never** be implemented in a UI shell:

- Encryption or decryption of any kind
- Key derivation or key management
- Vault config parsing or persistence
- Storage provider authentication or API calls
- Sync conflict resolution logic
- Vault tree manipulation
- File content transformations
- Password strength validation (beyond basic UX hints)
- Recovery key generation or verification

## Event-Driven State Flow

The core emits structured domain events. Clients subscribe and react.

```
User action (UI)
    │
    ▼
AppService method call
    │
    ▼
Core processes (vault/storage/sync/crypto)
    │
    ├──▶ Returns Result<T> to caller
    │
    └──▶ Emits AppEvent to all subscribers
              │
              ▼
         UI updates reactively
```

### Event Types

```rust
pub enum AppEvent {
    // Vault lifecycle
    VaultCreated { info: VaultInfo },
    VaultOpened { info: VaultInfo },
    VaultLocked,
    VaultClosed,

    // File operations
    EntryCreated { path: String, is_dir: bool },
    EntryDeleted { path: String },
    EntryModified { path: String },

    // Sync
    SyncStarted,
    SyncProgress { current: u64, total: u64 },
    SyncCompleted { changes: u32 },
    SyncConflict { path: String, resolution: ConflictResolution },
    SyncError { message: String },

    // Health
    HealthCheckCompleted { report: HealthReport },

    // Errors
    Error { code: ErrorCode, message: String },
}
```

Events flow through a broadcast channel. Each client holds a receiver. The UI thread polls or awaits events and applies state changes.

**macOS:** Events are marshaled to the main thread via `DispatchQueue.main.async` after FFI callback or polling.

**Linux:** Events are dispatched to the GLib main loop via `glib::MainContext::default().spawn_local()`.

## Bridge Strategies *(proposed)*

### macOS: C-ABI FFI (refines existing pattern)

The current `core/ffi` crate *(existing)* provides C functions callable from Swift. The facade would replace the ad-hoc FFI functions with a structured API:

```
Swift (VaultCore.swift)
    ↓ C function calls
core/ffi (thin C-ABI wrapper around AppService)
    ↓
axiomvault-app (AppService)
    ↓
vault / storage / sync / crypto
```

- `cbindgen` generates C headers from the FFI crate
- Opaque handles (`*mut AppHandle`) wrap `Arc<AppService>`
- JSON serialization for complex return types (directory listings, vault info)
- Thread-local error stack for error details
- Event polling: `axiom_poll_event()` returns next event as JSON, or null

### Linux: Direct Rust Linkage

The Linux client is a Rust binary that links `axiomvault-app` as a normal dependency:

```
GTK4 application (Rust)
    ↓ Direct function calls
axiomvault-app (AppService)
    ↓
vault / storage / sync / crypto
```

- No FFI overhead or serialization
- Native Rust types used directly
- Events consumed via `tokio::sync::broadcast::Receiver`
- GTK4 bindings via `gtk4-rs` crate

## FUSE Integration *(existing crate, proposed facade integration)*

Both macOS and Linux support mounting a vault as a virtual filesystem via `axiomvault-fuse` *(existing)*:

- **Linux:** Native `libfuse3` support
- **macOS:** `macFUSE` support
- Mount/unmount to be exposed through `AppService` methods *(proposed)*
- FUSE runs in a background thread; the facade would manage its lifecycle
- When FUSE is active, file operations through the mount point are equivalent to facade calls

FUSE is optional. Clients can operate entirely through the facade API without mounting.

## Data Flow: Vault on Disk *(existing)*

```
vault-root/
├── vault.config          # Encrypted metadata (salt, KDF, version, provider)
├── d/                    # Encrypted file content (UUID-named blobs)
│   ├── <uuid1>
│   └── <uuid2>
└── m/
    └── tree.json         # Encrypted directory tree index
```

All persistence is handled by the vault engine. Clients never read or write vault files directly.

## Future Compatibility

### Windows Client

The architecture supports a future Windows client without rework:

- **Bridge:** C-ABI FFI (same as macOS) or C++/WinRT interop
- **UI:** WinUI 3 or similar native framework
- **FUSE equivalent:** WinFsp or Dokan for virtual filesystem
- **Keyring:** Windows Credential Manager
- The `AppService` API is platform-agnostic; only the bridge and UI shell are new

### Local Daemon (optional, future)

The facade is designed so it could be hosted behind a local socket:

- `AppService` methods map cleanly to RPC-style request/response
- Events map to server-sent notifications
- A daemon would allow multiple frontends (CLI, GUI, tray agent) to share a single vault session
- Not required now; the architecture accommodates it without changes to core logic

### Mobile Clients

iOS and Android already consume the core via `core/ffi`. The new `axiomvault-app` facade can replace the current ad-hoc FFI functions, giving mobile clients the same structured API as desktop.

## Migration Path

The transition from the current state to this architecture:

1. **Create `axiomvault-app` crate** — wrap vault/storage/sync into a stateful facade with DTOs and events (#97, #98, #99, #100)
2. **Add contract tests** — verify facade behavior independent of any UI (#101)
3. **Refine FFI bridge** — point `core/ffi` at the facade instead of raw vault internals (#102)
4. **Adapt macOS client** — update `VaultCore.swift` to call the new facade API (#103)
5. **Build Linux skeleton** — GTK4 app consuming the facade directly (#104)
6. **Packaging** — platform-specific distribution (DMG, Flatpak/deb, etc.) (#105)

The Tauri desktop client (`clients/desktop`) remains functional during migration and can be deprecated once the native clients reach feature parity.

## Crate Dependency Graph (target state)

```
axiomvault-common
    ▲
    │
axiomvault-crypto
    ▲
    ├───────────────────┐
    │                   │
axiomvault-vault    axiomvault-storage
    ▲                   ▲
    │                   │
    ├───────┬───────────┘
    │       │
    │  axiomvault-sync
    │       ▲
    │       │
    ├───────┘
    │
axiomvault-fuse
    ▲
    │
axiomvault-app  ◄── NEW: application facade
    ▲
    ├────────────────────────┐
    │                        │
core/ffi                Linux GTK4 app
(C-ABI for macOS/mobile)    (direct link)
```
