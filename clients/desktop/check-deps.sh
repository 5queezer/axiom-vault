#!/bin/bash
# Check system dependencies for AxiomVault Desktop build
# This script verifies that all required development libraries are installed

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "Checking AxiomVault Desktop build dependencies..."
echo ""

MISSING_DEPS=()

check_pkg_config() {
    if ! command -v pkg-config &> /dev/null; then
        echo -e "${RED}✗ pkg-config is not installed${NC}"
        MISSING_DEPS+=("pkg-config")
        return 1
    fi
    echo -e "${GREEN}✓ pkg-config found${NC}"
    return 0
}

check_library() {
    local name=$1
    local pkg=$2
    local min_version=${3:-}

    if pkg-config --exists "$pkg" 2>/dev/null; then
        local version=$(pkg-config --modversion "$pkg" 2>/dev/null || echo "unknown")
        echo -e "${GREEN}✓ $name ($version)${NC}"
        return 0
    else
        echo -e "${RED}✗ $name not found${NC}"
        MISSING_DEPS+=("$name")
        return 1
    fi
}

# Check pkg-config first
check_pkg_config || true

echo ""
echo "Required libraries:"

# GTK3 and related
check_library "GTK 3" "gtk+-3.0" || true
check_library "GDK 3" "gdk-3.0" || true
check_library "GDK-Pixbuf" "gdk-pixbuf-2.0" || true
check_library "GLib 2.0" "glib-2.0" || true
check_library "Cairo" "cairo" || true
check_library "Pango" "pango" || true
check_library "ATK" "atk" || true

# WebKit
check_library "WebKit2GTK 4.1" "webkit2gtk-4.1" || true

# Optional but recommended
echo ""
echo "Optional libraries:"
check_library "libappindicator3" "ayatana-appindicator3-0.1" || check_library "libappindicator3" "appindicator3-0.1" || true
check_library "librsvg" "librsvg-2.0" || true
check_library "FUSE3" "fuse3" || true

echo ""

if [ ${#MISSING_DEPS[@]} -gt 0 ]; then
    echo -e "${RED}Missing dependencies detected!${NC}"
    echo ""
    echo "Please install the required packages:"
    echo ""

    # Detect package manager
    if command -v apt-get &> /dev/null; then
        echo -e "${YELLOW}For Debian/Ubuntu:${NC}"
        echo "sudo apt-get update"
        echo "sudo apt-get install -y \\"
        echo "    libgtk-3-dev \\"
        echo "    libwebkit2gtk-4.1-dev \\"
        echo "    libayatana-appindicator3-dev \\"
        echo "    librsvg2-dev \\"
        echo "    patchelf \\"
        echo "    libfuse3-dev"
    elif command -v dnf &> /dev/null; then
        echo -e "${YELLOW}For Fedora:${NC}"
        echo "sudo dnf install -y \\"
        echo "    gtk3-devel \\"
        echo "    webkit2gtk4.1-devel \\"
        echo "    libappindicator-gtk3-devel \\"
        echo "    librsvg2-devel \\"
        echo "    fuse3-devel"
    elif command -v pacman &> /dev/null; then
        echo -e "${YELLOW}For Arch Linux:${NC}"
        echo "sudo pacman -S --needed \\"
        echo "    gtk3 \\"
        echo "    webkit2gtk-4.1 \\"
        echo "    libappindicator-gtk3 \\"
        echo "    librsvg \\"
        echo "    fuse3"
    else
        echo "Please install GTK3, WebKit2GTK 4.1, and their development headers."
        echo "See clients/desktop/README.md for more information."
    fi

    echo ""
    exit 1
else
    echo -e "${GREEN}All required dependencies are installed!${NC}"
    echo "You can now build the desktop client with:"
    echo "  cargo build --package axiomvault-desktop"
    exit 0
fi
