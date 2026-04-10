# Packaging and Release Strategy

This document defines the packaging approach for AxiomVault's native desktop clients, covering runtime dependencies, distribution formats, signing, and release automation.

> **Status:** Living document. Updated as packaging infrastructure matures.

## Platform Matrix

| Platform | Client | Binary | Distribution formats |
|----------|--------|--------|---------------------|
| Linux x86_64 | GTK4/libadwaita | `axiomvault-gtk` | AppImage, `.deb`, Flatpak (future) |
| Linux aarch64 | GTK4/libadwaita | `axiomvault-gtk` | AppImage, `.deb` |
| macOS universal | SwiftUI/AppKit | `AxiomVault.app` | `.dmg`, direct `.app` zip |
| macOS (App Store) | SwiftUI/AppKit | `AxiomVault.app` | App Store (future) |
| CLI (all) | Terminal | `axiomvault` | `.tar.gz` (existing) |

## Linux

### Runtime Dependencies

The GTK4 client requires these shared libraries at runtime:

| Library | Min version | Package (Debian/Ubuntu) | Package (Fedora) |
|---------|-------------|------------------------|-------------------|
| GTK 4 | 4.12 | `libgtk-4-1` | `gtk4` |
| libadwaita | 1.4 | `libadwaita-1-0` | `libadwaita` |
| GLib | 2.76 | `libglib2.0-0` | `glib2` |

Optional runtime dependencies:

| Library | Purpose | Package (Debian/Ubuntu) |
|---------|---------|------------------------|
| libfuse3 | FUSE virtual filesystem | `libfuse3-3` |
| libsecret | Keyring integration | `libsecret-1-0` |
| xdg-desktop-portal | File picker, notifications | `xdg-desktop-portal-gtk` |

### Distribution Format Priority

**1. AppImage (primary, immediate)**

Self-contained bundle. No root required. Works on any Linux with a modern enough kernel and glibc.

- Embeds GTK4 and libadwaita libraries
- Single file download: `AxiomVault-x86_64.AppImage`
- Built using `linuxdeploy` + `linuxdeploy-plugin-gtk`
- Desktop integration via AppImageLauncher or manual `.desktop` file
- Suitable for GitHub Releases distribution

Constraints:
- Theming may not match host GTK theme perfectly (bundled GTK4)
- No auto-updates without external tooling (AppImageUpdate)
- File size larger due to bundled libraries (~50-80 MB estimated)

**2. `.deb` package (secondary, immediate)**

Native package for Debian/Ubuntu. Declares runtime dependencies, integrates with system package manager.

- Depends on system GTK4 and libadwaita (smaller download)
- Installs to `/usr/bin/axiomvault-gtk`
- Desktop entry, icon, and man page included
- Version tracking via dpkg
- Built using `cargo-deb`

Constraints:
- Requires separate packages per Ubuntu/Debian release (library versions)
- No auto-updates (user manages via apt)
- Minimum supported: Ubuntu 24.04 (ships GTK 4.14, libadwaita 1.5)

**3. Flatpak (future)**

Sandboxed distribution via Flathub. Good for discoverability and auto-updates.

- Runtime: `org.gnome.Platform` 46+ (includes GTK4, libadwaita)
- Permissions: filesystem access (vault directories), secret-service (keyring)
- Auto-updates via Flatpak/Flathub
- Sandboxing aligns with vault security posture

Constraints:
- Requires Flathub submission and review
- Portal-based file access (no direct filesystem — needs adaptation)
- FUSE mounting from sandbox may require portal integration

### XDG Conventions

The Linux client follows XDG Base Directory Specification:

| Directory | Purpose | Default |
|-----------|---------|---------|
| `$XDG_DATA_HOME/axiomvault/` | Vault metadata, logs | `~/.local/share/axiomvault/` |
| `$XDG_CONFIG_HOME/axiomvault/` | User preferences | `~/.config/axiomvault/` |
| `$XDG_CACHE_HOME/axiomvault/` | Temporary cache | `~/.cache/axiomvault/` |

Desktop entry: `com.axiomvault.gtk.desktop`

## macOS

### App Bundle Structure

```
AxiomVault.app/
  Contents/
    Info.plist
    MacOS/
      AxiomVault          # SwiftUI binary
    Frameworks/
      AxiomVaultCore.framework/  # Rust FFI (or embedded .dylib)
    Resources/
      AppIcon.icns
      Assets.car
    PlugIns/
      AxiomVaultFileProvider.appex/  # File Provider extension
```

### Signing and Notarization

**Development (current):**
- Unsigned builds via Xcode with automatic signing (local development only)
- No sandbox entitlements (stripped for free Apple ID compatibility)

**Distribution (planned):**
- Apple Developer Program membership required ($99/year)
- Code signing with Developer ID certificate
- Notarization via `xcrun notarytool` (required for Gatekeeper on macOS 10.15+)
- Hardened Runtime enabled for notarization compliance
- Entitlements restored: app-sandbox, file access, keychain

**App Store (future):**
- Bundle ID: `com.axiomvault.macos`
- Category: Utilities (`public.app-category.utilities`)
- App Sandbox required
- Export compliance declaration required (XChaCha20-Poly1305, see `APP_STORE.md`)
- File Provider extension for Finder integration

### Distribution Formats

**1. Direct download `.dmg` (primary)**

Standard macOS distribution. Drag-to-Applications experience.

- Built via `create-dmg` or `hdiutil`
- Signed and notarized `.app` inside
- Background image with install instructions
- Universal binary (arm64 + x86_64)

**2. Direct `.app` zip (secondary)**

For users who prefer direct download without `.dmg`.

- Signed and notarized
- Uploaded to GitHub Releases

**3. Homebrew Cask (future)**

```
brew install --cask axiomvault
```

Requires a Homebrew Cask tap or submission to homebrew-cask.

### Bridge / Runtime Packaging

The Rust core is compiled into a static library (`libaxiom_vault.a`) and bundled into an XCFramework. At build time, Xcode links this into the app binary. There is no separate runtime dependency — the Rust code is fully embedded.

For distribution:
- `AxiomVaultCore.xcframework` is built by `clients/apple/Scripts/build-apple.sh`
- The framework includes: static library + C header + module map
- Universal binaries are created via `lipo` for simulator (arm64 + x86_64) and macOS (arm64 + x86_64)

## Release Workflow

### Existing (CLI + iOS)

The current `release.yml` workflow handles:
- CLI binaries for Linux/macOS (x86_64 + arm64) as `.tar.gz`
- iOS XCFramework as `.zip`
- Source archive
- SHA256 checksums

Triggered by `v*` tags or manual dispatch.

### Additions for Native Clients

#### Linux GTK Client

New release job: build `axiomvault-gtk` on Ubuntu and package as AppImage and `.deb`.

```yaml
build-linux-gtk:
  name: Build Linux GTK - ${{ matrix.target }}
  runs-on: ubuntu-latest
  needs: create-release
  strategy:
    matrix:
      include:
        - target: x86_64-unknown-linux-gnu
          arch: amd64
  steps:
    - Install GTK4/libadwaita dev libraries
    - cargo build --release -p axiomvault-linux
    - Package as AppImage (linuxdeploy)
    - Package as .deb (cargo-deb)
    - Upload to GitHub Release
```

#### macOS App

New release job: build the Xcode project, sign, notarize, and package.

```yaml
build-macos-app:
  name: Build macOS App
  runs-on: macos-latest
  needs: create-release
  steps:
    - Build Rust XCFramework (release mode)
    - Generate Xcode project (xcodegen)
    - Build with xcodebuild -archivePath
    - Export .app from archive
    - Notarize (when signing is configured)
    - Create .dmg
    - Upload to GitHub Release
```

### Version Strategy

All crates and clients share a single version defined in the workspace `Cargo.toml`. Apple clients read from `project.yml` (`MARKETING_VERSION`). These must be kept in sync.

Release tags follow semver: `v0.1.0`, `v0.2.0-beta.1`, etc.

Pre-release detection (existing): tags containing `alpha`, `beta`, or `rc` are marked as pre-release on GitHub.

### Release Checklist

1. Update version in workspace `Cargo.toml`
2. Update `MARKETING_VERSION` in `clients/apple/project.yml`
3. Update `CHANGELOG.md` (or rely on auto-generated notes)
4. Tag: `git tag v0.x.0 && git push --tags`
5. Release workflow produces all artifacts
6. Review draft release, publish

## Platform Assumptions

### Linux
- GTK 4.12+ and libadwaita 1.4+ are available on Ubuntu 24.04+, Fedora 40+, Arch (rolling)
- Older distributions (Ubuntu 22.04) do not ship GTK4 — AppImage is the only option there
- Wayland is the primary display target; X11 compatibility via XWayland
- systemd is assumed for optional background service (future)

### macOS
- Minimum deployment target: macOS 13.0 (Ventura)
- Universal binaries (arm64 + x86_64) cover all current Macs
- Notarization is mandatory for non-App Store distribution since macOS 10.15
- File Provider extension requires sandbox entitlements and provisioning profile
- Hardened Runtime is required for notarization

### General
- The legacy Tauri desktop client has been removed
- Windows packaging is deferred (see architecture doc)
- Mobile distribution (iOS App Store, Google Play) is handled separately
