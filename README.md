# AxiomVault

A cross-platform encrypted vault system with client-side encryption, independent of cloud provider trust. Built in Rust with support for Linux, macOS, iOS, and Android.

## Features

- **Strong Encryption**: XChaCha20-Poly1305 with Argon2id key derivation
- **Cloud Storage**: Google Drive integration (iCloud, OneDrive planned)
- **FUSE Mounting**: Mount vaults as virtual filesystems
- **Cross-Platform**: CLI, desktop GUI, iOS, and Android clients
- **Sync Engine**: Conflict detection and resolution strategies

## Quick Start

### Prerequisites

**All Platforms:**
- Rust stable toolchain

**Linux (Debian/Ubuntu):**
```bash
sudo apt-get install -y libfuse3-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
    libappindicator3-dev librsvg2-dev patchelf
```

**macOS:**
```bash
brew install --cask macfuse
```

### Build & Install

```bash
# Clone the repository
git clone <repository-url>
cd axiom-vault

# Build the CLI tool
cargo build --release -p axiomvault-cli

# Optional: Add to PATH
sudo cp target/release/axiomvault /usr/local/bin/
```

### Basic Usage

#### Create a Vault

```bash
axiomvault create --name MyVault --path ~/my-vault
# Enter password when prompted
```

#### Add Files

```bash
axiomvault add --vault-path ~/my-vault \
    --source ~/documents/secret.pdf \
    --dest /secret.pdf
```

#### List Contents

```bash
axiomvault list --vault-path ~/my-vault
```

#### Extract Files

```bash
axiomvault extract --vault-path ~/my-vault \
    --source /secret.pdf \
    --dest ~/downloads/secret.pdf
```

#### Open Interactive Session

```bash
axiomvault open --path ~/my-vault
```

### Google Drive Integration

```bash
# Authenticate (opens browser)
axiomvault gdrive-auth --output ~/gdrive-tokens.json

# Create vault on Google Drive
axiomvault gdrive-create --name CloudVault \
    --folder-id YOUR_FOLDER_ID \
    --tokens ~/gdrive-tokens.json

# Open cloud vault
axiomvault gdrive-open --folder-id YOUR_FOLDER_ID \
    --tokens ~/gdrive-tokens.json
```

### Sync Operations

```bash
# Sync vault with remote
axiomvault sync --vault-path ~/my-vault --strategy keep-both

# Check sync status
axiomvault sync-status --vault-path ~/my-vault

# Configure automatic sync
axiomvault sync-configure --vault-path ~/my-vault \
    --mode periodic --interval 300
```

## Desktop Application

```bash
# Build desktop GUI (with dependency check)
make desktop

# Or use cargo directly
cargo build --package axiomvault-desktop

# With FUSE support
cargo build --package axiomvault-desktop --features axiomvault-fuse/fuse
```

See `make help` for all available build targets.

## CLI Reference

| Command | Description |
|---------|-------------|
| `create` | Create a new encrypted vault |
| `open` | Open/unlock vault interactively |
| `info` | Display vault information |
| `list` | List vault contents |
| `add` | Add file to vault |
| `extract` | Extract file from vault |
| `mkdir` | Create directory in vault |
| `remove` | Remove file/directory |
| `change-password` | Change vault password |
| `gdrive-auth` | Authenticate with Google Drive |
| `gdrive-create` | Create vault on Google Drive |
| `gdrive-open` | Open vault from Google Drive |
| `sync` | Synchronize vault |
| `sync-status` | Show sync status |
| `sync-configure` | Configure sync behavior |

### Global Options

```bash
-v, --verbose    Enable debug logging
--help           Show help information
```

### KDF Strength Levels

```bash
--strength interactive  # Fast, mobile-friendly
--strength moderate     # Balanced (default)
--strength sensitive    # High security, slower
```

## Project Structure

```
axiom-vault/
├── core/                 # Core library modules
│   ├── crypto/          # Encryption & key derivation
│   ├── vault/           # Vault management
│   ├── storage/         # Storage providers (Google Drive)
│   ├── sync/            # Sync engine
│   ├── fuse/            # FUSE filesystem
│   └── ffi/             # Mobile FFI bindings
├── clients/
│   ├── ios/             # SwiftUI iOS app
│   ├── android/         # Kotlin Android app
│   └── desktop/         # Tauri desktop GUI
└── tools/cli/           # Command-line interface
```

## Development

```bash
# Format code
cargo fmt --all

# Run linter
cargo clippy --all -- -D warnings

# Run tests
cargo test --all

# Build all packages
cargo build --workspace
```

## Environment Variables

```bash
RUST_LOG=debug           # Set logging level (info, debug, trace)
RUST_BACKTRACE=1         # Enable backtraces for debugging
```

## Security

- Client-side encryption: data encrypted before leaving your device
- Memory zeroization: sensitive data wiped from memory
- No plaintext logging: secrets never appear in logs
- Constant-time comparisons for cryptographic operations

## License

MIT OR Apache-2.0
