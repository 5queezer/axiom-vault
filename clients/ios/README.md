# AxiomVault iOS Client

iOS client for AxiomVault encrypted file storage system.

## Features

- Create and manage encrypted vaults
- SwiftUI-based modern interface
- Biometric authentication (Face ID / Touch ID)
- Google Drive integration via OAuth2
- Background synchronization
- File import/export with encryption

## Requirements

- iOS 16.0+
- Xcode 15.0+
- Rust toolchain with iOS targets
- macOS (for building)

## Building

### 1. Install Rust iOS Targets

```bash
rustup target add aarch64-apple-ios
rustup target add aarch64-apple-ios-sim
rustup target add x86_64-apple-ios
```

### 2. Build Rust Static Library

```bash
cd Scripts
./build-ios.sh
```

This will:
- Build the Rust FFI layer for iOS device and simulator
- Create a universal binary for simulators
- Generate an XCFramework

### 3. Create Xcode Project

1. Open Xcode and create a new iOS App project
2. Choose SwiftUI as the interface
3. Set the bundle identifier to `com.axiomvault.ios`
4. Add the generated XCFramework to your project

### 4. Configure Xcode Project

#### Add Bridging Header

1. Add `AxiomVault-Bridging-Header.h` to your project
2. Set "Objective-C Bridging Header" in Build Settings to the header path

#### Link Static Library

1. Add the XCFramework to "Frameworks, Libraries, and Embedded Content"
2. Set "Embed" to "Do Not Embed" (static library)

#### Additional Linker Flags

Add to "Other Linker Flags" in Build Settings:
- `-lresolv`
- `-lSystem`
- `-lc++`

#### Enable Capabilities

1. Enable "Background Modes":
   - Background fetch
   - Background processing
2. Enable "Keychain Sharing" for biometric password storage

### 5. Configure Google Drive OAuth

1. Create a project in Google Cloud Console
2. Enable Google Drive API
3. Create OAuth 2.0 credentials (iOS app)
4. Update `GoogleDriveAuth.swift` with your client ID:

```swift
GoogleDriveAuth.shared.configure(
    clientId: "YOUR_CLIENT_ID.apps.googleusercontent.com"
)
```

5. Add URL scheme to Info.plist (already configured)

## Project Structure

```
AxiomVault/
├── Sources/
│   ├── AxiomVaultApp.swift      # App entry point
│   ├── Core/
│   │   ├── VaultCore.swift      # FFI wrapper
│   │   └── AxiomVault-Bridging-Header.h
│   ├── Models/
│   │   └── VaultManager.swift   # State management
│   ├── Views/
│   │   ├── ContentView.swift
│   │   ├── CreateVaultView.swift
│   │   ├── OpenVaultView.swift
│   │   └── VaultBrowserView.swift
│   └── Services/
│       ├── BiometricAuth.swift   # Face ID/Touch ID
│       ├── GoogleDriveAuth.swift # OAuth2
│       └── BackgroundSync.swift  # BGTaskScheduler
├── Resources/
│   └── Info.plist
└── Frameworks/
    └── AxiomVaultCore.xcframework  # Built by script
```

## Usage

### Creating a Vault

```swift
let vaultManager = VaultManager()
await vaultManager.createVault(name: "MyVault", password: "securePassword123")
```

### Opening a Vault

```swift
await vaultManager.openVault(at: vaultPath, password: "securePassword123")
```

### Adding Files

```swift
await vaultManager.addFile(from: localFileURL)
```

### Biometric Unlock

```swift
let biometric = BiometricAuth.shared
if biometric.isBiometricAvailable {
    // Store password after initial unlock
    try biometric.storePassword(password, for: vaultPath)

    // Retrieve password using biometrics
    if let password = try await biometric.retrievePassword(for: vaultPath) {
        await vaultManager.openVault(at: vaultPath, password: password)
    }
}
```

### Google Drive Sync

```swift
let gdrive = GoogleDriveAuth.shared
gdrive.configure(clientId: "YOUR_CLIENT_ID")

do {
    let tokens = try await gdrive.authenticate()
    // Use tokens for API calls
} catch {
    print("Authentication failed: \(error)")
}
```

## Security Considerations

1. **Password Storage**: Passwords are only stored in Keychain with biometric protection
2. **Key Material**: All encryption keys are managed by the Rust core and zeroized on drop
3. **Memory Safety**: The Rust FFI provides memory-safe operations
4. **No Plaintext**: File content is encrypted before leaving the Rust core

## Known Limitations

1. File Provider Extension not yet implemented
2. Google Drive sync is placeholder (requires Rust sync FFI integration)
3. Conflict resolution UI not complete
4. Large file handling may need optimization

## Development

### Testing on Simulator

```bash
# Build for simulator
cargo build --release --target aarch64-apple-ios-sim -p axiom-ffi

# Run in Xcode simulator
```

### Debugging

1. Enable Rust debugging in Xcode by setting `RUST_BACKTRACE=1`
2. Check `axiom_last_error()` for detailed error messages
3. Use Instruments for performance profiling

## License

MIT OR Apache-2.0
