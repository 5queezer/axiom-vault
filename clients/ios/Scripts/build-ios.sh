#!/bin/bash
#
# Build script for AxiomVault iOS static library
#
# This script builds the Rust core as a static library for iOS targets
# and creates a universal binary (xcframework) for distribution.
#

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
FFI_CRATE_DIR="$PROJECT_ROOT/core/ffi"
BUILD_DIR="$PROJECT_ROOT/target"
OUTPUT_DIR="$SCRIPT_DIR/../Frameworks"

# iOS targets
IOS_ARCH="aarch64-apple-ios"
IOS_SIM_ARCH="aarch64-apple-ios-sim"
IOS_SIM_X86="x86_64-apple-ios"

# Library name
LIB_NAME="libaxiom_vault"

echo -e "${GREEN}=== AxiomVault iOS Build Script ===${NC}"
echo "Project root: $PROJECT_ROOT"
echo "Output directory: $OUTPUT_DIR"

# Check for Rust installation
if ! command -v rustup &> /dev/null; then
    echo -e "${RED}Error: rustup is not installed${NC}"
    echo "Please install Rust from https://rustup.rs/"
    exit 1
fi

# Check for cargo
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}Error: cargo is not found${NC}"
    exit 1
fi

# Ensure we're using the stable toolchain as specified in rust-toolchain.toml
# and install iOS targets for that specific toolchain
echo -e "${YELLOW}Installing iOS Rust targets for stable toolchain...${NC}"

# First, ensure the stable toolchain is installed
rustup toolchain install stable --no-self-update 2>/dev/null || true

# Install targets explicitly for the stable toolchain (which matches rust-toolchain.toml)
rustup target add --toolchain stable $IOS_ARCH
if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to install $IOS_ARCH target for stable toolchain${NC}"
    exit 1
fi

rustup target add --toolchain stable $IOS_SIM_ARCH
if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to install $IOS_SIM_ARCH target for stable toolchain${NC}"
    exit 1
fi

rustup target add --toolchain stable $IOS_SIM_X86
if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to install $IOS_SIM_X86 target for stable toolchain${NC}"
    exit 1
fi

# Verify targets are installed for the stable toolchain
echo -e "${YELLOW}Verifying Rust targets are available for stable toolchain...${NC}"
if ! rustup target list --toolchain stable --installed | grep -q "$IOS_ARCH"; then
    echo -e "${RED}Error: $IOS_ARCH target is not installed for stable toolchain${NC}"
    echo "Try running: rustup target add --toolchain stable $IOS_ARCH"
    exit 1
fi
if ! rustup target list --toolchain stable --installed | grep -q "$IOS_SIM_ARCH"; then
    echo -e "${RED}Error: $IOS_SIM_ARCH target is not installed for stable toolchain${NC}"
    echo "Try running: rustup target add --toolchain stable $IOS_SIM_ARCH"
    exit 1
fi
if ! rustup target list --toolchain stable --installed | grep -q "$IOS_SIM_X86"; then
    echo -e "${RED}Error: $IOS_SIM_X86 target is not installed for stable toolchain${NC}"
    echo "Try running: rustup target add --toolchain stable $IOS_SIM_X86"
    exit 1
fi
echo -e "${GREEN}All iOS targets verified for stable toolchain${NC}"

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Build for iOS device (arm64)
echo -e "${YELLOW}Building for iOS device (arm64)...${NC}"
cd "$PROJECT_ROOT"
cargo build --release --target $IOS_ARCH -p axiom-ffi

if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to build for iOS device${NC}"
    exit 1
fi

# Build for iOS simulator (arm64)
echo -e "${YELLOW}Building for iOS simulator (arm64)...${NC}"
cargo build --release --target $IOS_SIM_ARCH -p axiom-ffi

if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to build for iOS simulator (arm64)${NC}"
    exit 1
fi

# Build for iOS simulator (x86_64)
echo -e "${YELLOW}Building for iOS simulator (x86_64)...${NC}"
cargo build --release --target $IOS_SIM_X86 -p axiom-ffi

if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to build for iOS simulator (x86_64)${NC}"
    exit 1
fi

# Create directories for each platform
IOS_DEVICE_DIR="$OUTPUT_DIR/ios-device"
IOS_SIM_DIR="$OUTPUT_DIR/ios-simulator"

mkdir -p "$IOS_DEVICE_DIR"
mkdir -p "$IOS_SIM_DIR"

# Copy device library
echo -e "${YELLOW}Copying iOS device library...${NC}"
cp "$BUILD_DIR/$IOS_ARCH/release/$LIB_NAME.a" "$IOS_DEVICE_DIR/"

# Create universal simulator library (combines arm64 and x86_64)
echo -e "${YELLOW}Creating universal simulator library...${NC}"
lipo -create \
    "$BUILD_DIR/$IOS_SIM_ARCH/release/$LIB_NAME.a" \
    "$BUILD_DIR/$IOS_SIM_X86/release/$LIB_NAME.a" \
    -output "$IOS_SIM_DIR/$LIB_NAME.a"

# Copy header file
echo -e "${YELLOW}Copying C header...${NC}"
HEADER_DIR="$OUTPUT_DIR/Headers"
mkdir -p "$HEADER_DIR"

if [ -f "$BUILD_DIR/include/axiom_ffi.h" ]; then
    cp "$BUILD_DIR/include/axiom_ffi.h" "$HEADER_DIR/"
else
    echo -e "${YELLOW}Warning: C header not found, using bridging header${NC}"
fi

# Create module map
echo -e "${YELLOW}Creating module map...${NC}"
cat > "$HEADER_DIR/module.modulemap" << EOF
module AxiomVaultCore {
    header "axiom_ffi.h"
    export *
}
EOF

# Create xcframework
echo -e "${YELLOW}Creating XCFramework...${NC}"
XCFRAMEWORK_PATH="$OUTPUT_DIR/AxiomVaultCore.xcframework"

# Remove existing xcframework
rm -rf "$XCFRAMEWORK_PATH"

xcodebuild -create-xcframework \
    -library "$IOS_DEVICE_DIR/$LIB_NAME.a" \
    -headers "$HEADER_DIR" \
    -library "$IOS_SIM_DIR/$LIB_NAME.a" \
    -headers "$HEADER_DIR" \
    -output "$XCFRAMEWORK_PATH"

if [ $? -ne 0 ]; then
    echo -e "${YELLOW}Warning: Failed to create xcframework, libraries are still available${NC}"
else
    echo -e "${GREEN}XCFramework created at: $XCFRAMEWORK_PATH${NC}"
fi

# Clean up intermediate directories (optional)
# rm -rf "$IOS_DEVICE_DIR" "$IOS_SIM_DIR"

# Print summary
echo ""
echo -e "${GREEN}=== Build Complete ===${NC}"
echo "Static libraries built for:"
echo "  - iOS device (arm64): $IOS_DEVICE_DIR/$LIB_NAME.a"
echo "  - iOS simulator (arm64 + x86_64): $IOS_SIM_DIR/$LIB_NAME.a"
echo ""
echo "To use in Xcode project:"
echo "1. Add the xcframework to your project"
echo "2. Link with the static library"
echo "3. Add bridging header import"
echo ""
echo -e "${GREEN}Done!${NC}"
