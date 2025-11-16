# AxiomVault Desktop Client

Desktop client for AxiomVault with FUSE filesystem mounting support.

## Features

- Create and manage encrypted vaults
- FUSE filesystem mounting (Linux and macOS)
- SQLite-based local index for fast metadata caching
- Google Drive integration (via storage provider)
- Modern GUI built with Tauri

## System Requirements

### Linux

```bash
# Debian/Ubuntu
sudo apt-get install -y \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libappindicator3-dev \
    librsvg2-dev \
    patchelf \
    libfuse3-dev

# Fedora
sudo dnf install -y \
    gtk3-devel \
    webkit2gtk4.1-devel \
    libappindicator-gtk3-devel \
    librsvg2-devel \
    fuse3-devel

# Arch Linux
sudo pacman -S --needed \
    gtk3 \
    webkit2gtk-4.1 \
    libappindicator-gtk3 \
    librsvg \
    fuse3
```

### macOS

```bash
# Install macFUSE from https://osxfuse.github.io/
# Or using Homebrew:
brew install --cask macfuse
```

## Building

```bash
# Build without FUSE support
cargo build --package axiomvault-desktop

# Build with FUSE support (requires libfuse3-dev)
cargo build --package axiomvault-desktop --features axiomvault-fuse/fuse

# Development build
cd clients/desktop/src-tauri
cargo build
```

## Running

```bash
# Run the desktop application
cargo run --package axiomvault-desktop
```

## Architecture

```
src-tauri/
├── src/
│   ├── main.rs           # Application entry point
│   ├── commands.rs       # Tauri command handlers
│   ├── local_index.rs    # SQLite metadata caching
│   └── state.rs          # Application state management
├── Cargo.toml            # Rust dependencies
└── tauri.conf.json       # Tauri configuration

src/
└── index.html            # Frontend UI
```

## FUSE Mount

The desktop client supports mounting vaults as FUSE filesystems:

1. Create or unlock a vault
2. Click "Mount" button
3. Specify mount point (e.g., `/mnt/vault` or `/tmp/myvault`)
4. Access encrypted files through the mounted filesystem

Files are automatically decrypted when read and encrypted when written.

### FUSE Feature Flag

FUSE support is optional and requires system libraries:

- Linux: `libfuse3-dev`
- macOS: macFUSE

Build with FUSE support:
```bash
cargo build --features axiomvault-fuse/fuse
```

Without FUSE support, the mount functionality will return an error indicating the feature is not available.

## Local Index

The application maintains a SQLite database for each vault to cache metadata:

- Fast startup times
- Offline browsing of vault structure
- Sync status tracking
- Stored in `~/.local/share/axiomvault/` (Linux) or `~/Library/Application Support/axiomvault/` (macOS)

## Security

- Master keys are kept in memory only while vault is unlocked
- Keys are automatically zeroized on lock or application exit
- No plaintext file content touches disk
- Local index stores only encrypted names and metadata
