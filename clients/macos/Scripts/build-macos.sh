#!/bin/bash
set -euo pipefail

# Build AxiomVaultCore.xcframework for macOS (arm64 + x86_64)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKSPACE_DIR="$(cd "$PROJECT_DIR/../.." && pwd)"
FFI_CRATE="$WORKSPACE_DIR/core/ffi"
FRAMEWORK_DIR="$PROJECT_DIR/Frameworks"
BUILD_DIR="$PROJECT_DIR/build"

echo "=== AxiomVault macOS Framework Build ==="
echo "Workspace: $WORKSPACE_DIR"
echo "Output:    $FRAMEWORK_DIR"
echo ""

# Check Rust toolchain
if ! command -v cargo &> /dev/null; then
    echo "error: Rust toolchain not found. Install from https://rustup.rs"
    exit 1
fi

# Ensure macOS targets are installed
TARGETS=(
    "aarch64-apple-darwin"
    "x86_64-apple-darwin"
)

for target in "${TARGETS[@]}"; do
    if ! rustup target list --installed | grep -q "$target"; then
        echo "Installing Rust target: $target"
        rustup target add "$target"
    fi
done

# Clean previous build artifacts
rm -rf "$BUILD_DIR"
rm -rf "$FRAMEWORK_DIR/AxiomVaultCore.xcframework"
mkdir -p "$BUILD_DIR"

# Build for each target
for target in "${TARGETS[@]}"; do
    echo ""
    echo "--- Building for $target ---"
    cargo build \
        --manifest-path "$FFI_CRATE/Cargo.toml" \
        --target "$target" \
        --release \
        --quiet

    TARGET_LIB="$WORKSPACE_DIR/target/$target/release/libaxiom_vault_ffi.a"
    if [ ! -f "$TARGET_LIB" ]; then
        echo "error: Build output not found: $TARGET_LIB"
        exit 1
    fi

    mkdir -p "$BUILD_DIR/$target"
    cp "$TARGET_LIB" "$BUILD_DIR/$target/"
    echo "Built: $TARGET_LIB"
done

# Create universal binary
echo ""
echo "--- Creating universal macOS binary ---"
mkdir -p "$BUILD_DIR/macos-universal"
lipo -create \
    "$BUILD_DIR/aarch64-apple-darwin/libaxiom_vault_ffi.a" \
    "$BUILD_DIR/x86_64-apple-darwin/libaxiom_vault_ffi.a" \
    -output "$BUILD_DIR/macos-universal/libaxiom_vault_ffi.a"
echo "Created universal binary"

# Generate C header via cbindgen (if available)
HEADER_DIR="$BUILD_DIR/include"
mkdir -p "$HEADER_DIR"

if command -v cbindgen &> /dev/null; then
    echo ""
    echo "--- Generating C header ---"
    cbindgen \
        --config "$FFI_CRATE/cbindgen.toml" \
        --crate axiom-vault-ffi \
        --output "$HEADER_DIR/axiom_vault_ffi.h" \
        "$FFI_CRATE"
    echo "Generated: $HEADER_DIR/axiom_vault_ffi.h"
else
    echo "warning: cbindgen not found, using bridging header directly"
fi

# Create XCFramework
echo ""
echo "--- Creating XCFramework ---"
mkdir -p "$FRAMEWORK_DIR"

xcodebuild -create-xcframework \
    -library "$BUILD_DIR/macos-universal/libaxiom_vault_ffi.a" \
    -headers "$HEADER_DIR" \
    -output "$FRAMEWORK_DIR/AxiomVaultCore.xcframework"

echo ""
echo "=== Build complete ==="
echo "XCFramework: $FRAMEWORK_DIR/AxiomVaultCore.xcframework"
