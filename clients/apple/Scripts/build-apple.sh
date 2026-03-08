#!/bin/bash
set -euo pipefail

# Build AxiomVaultCore.xcframework for all Apple platforms
# Usage: ./build-apple.sh [--platform ios|macos|all]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKSPACE_DIR="$(cd "$PROJECT_DIR/../.." && pwd)"
FFI_CRATE="$WORKSPACE_DIR/core/ffi"
BUILD_DIR="$WORKSPACE_DIR/target"
FRAMEWORK_DIR="$PROJECT_DIR/Frameworks"
STAGING_DIR="$PROJECT_DIR/build"

# Parse args
PLATFORM="${1:-all}"
case "$PLATFORM" in
    --platform) PLATFORM="${2:-all}" ;;
esac

echo "=== AxiomVault Apple Framework Build ==="
echo "Workspace: $WORKSPACE_DIR"
echo "Platform:  $PLATFORM"
echo "Output:    $FRAMEWORK_DIR"
echo ""

# Check Rust toolchain
if ! command -v rustup &> /dev/null; then
    echo "error: Rust toolchain not found. Install from https://rustup.rs"
    exit 1
fi

# Library name (from Cargo.toml: name = "axiom_vault")
LIB_NAME="libaxiom_vault"

# Get cargo path (prefer rustup's cargo over Homebrew)
CARGO="$HOME/.cargo/bin/cargo"
if [ ! -f "$CARGO" ]; then
    CARGO="$(which cargo)"
fi

# Ensure stable toolchain
rustup toolchain install stable --no-self-update 2>/dev/null || true

# Define targets per platform
IOS_TARGETS=("aarch64-apple-ios" "aarch64-apple-ios-sim" "x86_64-apple-ios")
MACOS_TARGETS=("aarch64-apple-darwin" "x86_64-apple-darwin")

TARGETS=()
case "$PLATFORM" in
    ios)   TARGETS=("${IOS_TARGETS[@]}") ;;
    macos) TARGETS=("${MACOS_TARGETS[@]}") ;;
    all)   TARGETS=("${IOS_TARGETS[@]}" "${MACOS_TARGETS[@]}") ;;
    *)     echo "error: Unknown platform '$PLATFORM'. Use ios, macos, or all."; exit 1 ;;
esac

# Install Rust targets
echo "--- Installing Rust targets ---"
for target in "${TARGETS[@]}"; do
    rustup target add --toolchain stable "$target" 2>/dev/null || true
done

# Clean staging
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"

# Build each target
for target in "${TARGETS[@]}"; do
    echo ""
    echo "--- Building for $target ---"
    "$CARGO" +stable build \
        --manifest-path "$FFI_CRATE/Cargo.toml" \
        --target "$target" \
        --release \
        -p axiom-ffi

    TARGET_LIB="$BUILD_DIR/$target/release/$LIB_NAME.a"
    if [ ! -f "$TARGET_LIB" ]; then
        echo "error: Build output not found: $TARGET_LIB"
        exit 1
    fi

    mkdir -p "$STAGING_DIR/$target"
    cp "$TARGET_LIB" "$STAGING_DIR/$target/"
    echo "Built: $TARGET_LIB"
done

# Generate C header
HEADER_DIR="$STAGING_DIR/include"
mkdir -p "$HEADER_DIR"

if command -v cbindgen &> /dev/null; then
    echo ""
    echo "--- Generating C header ---"
    cbindgen \
        --config "$FFI_CRATE/cbindgen.toml" \
        --crate axiom-ffi \
        --output "$HEADER_DIR/axiom_vault.h" \
        "$FFI_CRATE" 2>/dev/null || true
fi

# Fallback: create minimal module map if header wasn't generated
if [ ! -f "$HEADER_DIR/axiom_vault.h" ]; then
    echo "warning: cbindgen header not generated, using bridging header directly"
    cp "$PROJECT_DIR/Shared/Sources/Core/AxiomVault-Bridging-Header.h" "$HEADER_DIR/axiom_vault.h"
fi

cat > "$HEADER_DIR/module.modulemap" << 'EOF'
module AxiomVaultCore {
    header "axiom_vault.h"
    export *
}
EOF

# Create platform libraries
echo ""
echo "--- Creating platform libraries ---"

XCFRAMEWORK_ARGS=()

# iOS device
if [ "$PLATFORM" = "ios" ] || [ "$PLATFORM" = "all" ]; then
    if [ -f "$STAGING_DIR/aarch64-apple-ios/$LIB_NAME.a" ]; then
        mkdir -p "$STAGING_DIR/ios-device"
        cp "$STAGING_DIR/aarch64-apple-ios/$LIB_NAME.a" "$STAGING_DIR/ios-device/"
        XCFRAMEWORK_ARGS+=(-library "$STAGING_DIR/ios-device/$LIB_NAME.a" -headers "$HEADER_DIR")
    fi

    # iOS simulator (universal arm64 + x86_64)
    if [ -f "$STAGING_DIR/aarch64-apple-ios-sim/$LIB_NAME.a" ] && [ -f "$STAGING_DIR/x86_64-apple-ios/$LIB_NAME.a" ]; then
        mkdir -p "$STAGING_DIR/ios-simulator"
        lipo -create \
            "$STAGING_DIR/aarch64-apple-ios-sim/$LIB_NAME.a" \
            "$STAGING_DIR/x86_64-apple-ios/$LIB_NAME.a" \
            -output "$STAGING_DIR/ios-simulator/$LIB_NAME.a"
        XCFRAMEWORK_ARGS+=(-library "$STAGING_DIR/ios-simulator/$LIB_NAME.a" -headers "$HEADER_DIR")
    fi
fi

# macOS (universal arm64 + x86_64)
if [ "$PLATFORM" = "macos" ] || [ "$PLATFORM" = "all" ]; then
    if [ -f "$STAGING_DIR/aarch64-apple-darwin/$LIB_NAME.a" ] && [ -f "$STAGING_DIR/x86_64-apple-darwin/$LIB_NAME.a" ]; then
        mkdir -p "$STAGING_DIR/macos-universal"
        lipo -create \
            "$STAGING_DIR/aarch64-apple-darwin/$LIB_NAME.a" \
            "$STAGING_DIR/x86_64-apple-darwin/$LIB_NAME.a" \
            -output "$STAGING_DIR/macos-universal/$LIB_NAME.a"
        XCFRAMEWORK_ARGS+=(-library "$STAGING_DIR/macos-universal/$LIB_NAME.a" -headers "$HEADER_DIR")
    fi
fi

# Create XCFramework
echo ""
echo "--- Creating XCFramework ---"
mkdir -p "$FRAMEWORK_DIR"
rm -rf "$FRAMEWORK_DIR/AxiomVaultCore.xcframework"

xcodebuild -create-xcframework \
    "${XCFRAMEWORK_ARGS[@]}" \
    -output "$FRAMEWORK_DIR/AxiomVaultCore.xcframework"

echo ""
echo "=== Build complete ==="
echo "XCFramework: $FRAMEWORK_DIR/AxiomVaultCore.xcframework"
