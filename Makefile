# AxiomVault Build & Install Helpers

# Installation variables (can be overridden)
PREFIX ?= /usr/local
DESTDIR ?= /
BINDIR = $(DESTDIR)$(PREFIX)/bin
CARGO ?= $(HOME)/.cargo/bin/cargo

# Application info
LINUX_APP_NAME = axiomvault-gtk
LINUX_APP_BINARY = target/release/$(LINUX_APP_NAME)

.PHONY: all cli core install uninstall clean-install help
.PHONY: linux linux-release check-linux-deps install-linux uninstall-linux
.PHONY: ios ios-framework ios-project check-ios-deps
.PHONY: macos macos-framework macos-project check-macos-deps
.PHONY: apple apple-framework apple-project check-apple-deps

# Default target
all: help

help:
	@echo "AxiomVault Build & Install Targets"
	@echo "===================================="
	@echo ""
	@echo "Build Targets:"
	@echo "  make linux           - Build native Linux GTK4 client"
	@echo "  make linux-release   - Build Linux GTK4 client in release mode"
	@echo "  make cli             - Build CLI tool"
	@echo "  make core            - Build core libraries"
	@echo ""
	@echo "Install Targets:"
	@echo "  make install         - Build and install Linux GTK4 client"
	@echo "  make install-linux   - Build and install Linux GTK4 client"
	@echo "  make uninstall       - Remove installed Linux GTK4 client"
	@echo ""
	@echo "Advanced Install:"
	@echo "  PREFIX=/opt/axiom make install  - Install to /opt/axiom"
	@echo "  DESTDIR=/staging make install   - Stage install to /staging"
	@echo ""
	@echo "FUSE Support:"
	@if [ -n "$(FUSE_FEATURE)" ]; then \
		echo "  ✓ FUSE detected — vault mount support will be compiled in"; \
	else \
		echo "  ✗ FUSE not found — install libfuse3-dev for vault mount support"; \
	fi
	@echo ""
	@echo "Troubleshooting:"
	@echo "  sudo make uninstall && sudo make install  - Full reinstall"
	@echo "  sudo kbuildsycoca5 --noincremental      - Force KDE cache update"
	@echo ""
	@echo "Apple (unified iOS + macOS):"
	@echo "  make apple           - Build framework for all platforms + generate Xcode project"
	@echo "  make apple-framework - Build Rust XCFramework for iOS + macOS"
	@echo "  make apple-project   - Generate Xcode project (3 targets)"
	@echo "  make ios             - Build iOS-only framework + project"
	@echo "  make macos           - Build macOS-only framework + project"
	@echo ""
	@echo "Cleanup:"
	@echo "  make clean-install   - Remove all installed files"
	@echo ""

# Auto-detect FUSE support
FUSE_FEATURE :=
ifeq ($(shell uname),Darwin)
  ifneq ($(wildcard /Library/Frameworks/macFUSE.framework),)
    FUSE_FEATURE := --features fuse
  else ifneq ($(wildcard /usr/local/include/fuse/fuse.h),)
    FUSE_FEATURE := --features fuse
  else ifneq ($(wildcard /opt/homebrew/include/fuse/fuse.h),)
    FUSE_FEATURE := --features fuse
  endif
else ifeq ($(shell uname),Linux)
  ifneq ($(shell pkg-config --exists fuse3 2>/dev/null && echo yes),)
    FUSE_FEATURE := --features fuse
  else ifneq ($(shell pkg-config --exists fuse 2>/dev/null && echo yes),)
    FUSE_FEATURE := --features fuse
  endif
endif

# Native Linux GTK4 client
linux: check-linux-deps
	$(CARGO) build --package axiomvault-linux

linux-release: check-linux-deps
	$(CARGO) build --package axiomvault-linux --release

# CLI tool
cli:
	$(CARGO) build --package axiomvault-cli

# Core libraries
core:
	$(CARGO) build --workspace --exclude axiomvault-cli

# Install targets
install: install-linux
	@echo ""
	@echo "✅ AxiomVault installed successfully!"
	@echo "Launch with: axiomvault"
	@echo "Or run the Linux GTK binary directly: axiomvault-gtk"

uninstall: uninstall-linux
	@echo "✅ AxiomVault uninstalled"

clean-install: uninstall
	@echo "Cleaned up all installation files"

# Linux GTK4 install targets
install-linux: linux-release
	@echo "Installing Linux GTK4 client..."
	@mkdir -p $(BINDIR)
	@install -m 755 $(LINUX_APP_BINARY) $(BINDIR)/$(LINUX_APP_NAME)
	@ln -sf $(BINDIR)/$(LINUX_APP_NAME) $(BINDIR)/axiomvault
	@echo "✓ Binary installed to $(BINDIR)/$(LINUX_APP_NAME)"
	@echo "✓ Symlinked $(BINDIR)/axiomvault -> $(LINUX_APP_NAME)"

uninstall-linux:
	@echo "Removing Linux GTK4 client..."
	@rm -f $(BINDIR)/$(LINUX_APP_NAME)
	@rm -f $(BINDIR)/axiomvault
	@echo "✓ Removed"

# Check native Linux GTK4 dependencies
check-linux-deps:
	@if [ "$$(uname)" != "Linux" ]; then \
		echo "ERROR: Linux GTK4 client requires Linux"; \
		exit 1; \
	fi
	@echo "Checking GTK4/libadwaita dependencies..."
	@command -v pkg-config >/dev/null 2>&1 || { \
		echo "ERROR: pkg-config not found"; \
		echo "Install with: sudo apt-get install pkg-config"; \
		exit 1; \
	}
	@pkg-config --exists gtk4 2>/dev/null || { \
		echo "ERROR: GTK 4 not found"; \
		echo "Install with: sudo apt-get install libgtk-4-dev"; \
		exit 1; \
	}
	@pkg-config --exists libadwaita-1 2>/dev/null || { \
		echo "ERROR: libadwaita not found"; \
		echo "Install with: sudo apt-get install libadwaita-1-dev"; \
		exit 1; \
	}
	@echo "✓ All GTK4 dependencies found"

# Apple unified targets (iOS + macOS in one Xcode project)
apple: apple-framework apple-project
	@echo "Apple project ready! Open clients/apple/AxiomVault.xcodeproj in Xcode"
	@echo "Schemes: AxiomVault-iOS, AxiomVault-macOS"

apple-framework: check-apple-deps
	@echo "Building Rust XCFramework for all Apple platforms..."
	@cd clients/apple/Scripts && ./build-apple.sh

apple-project: check-apple-deps
	@echo "Generating Xcode project with XcodeGen..."
	@cd clients/apple && xcodegen generate
	@echo "✓ Generated AxiomVault.xcodeproj"

# Platform-specific shortcuts (still use unified project)
ios: check-apple-deps
	@echo "Building Rust XCFramework for iOS..."
	@cd clients/apple/Scripts && ./build-apple.sh --platform ios
	@cd clients/apple && xcodegen generate
	@echo "iOS ready! Open clients/apple/AxiomVault.xcodeproj and select AxiomVault-iOS scheme"

macos: check-apple-deps
	@echo "Building Rust XCFramework for macOS..."
	@cd clients/apple/Scripts && ./build-apple.sh --platform macos
	@cd clients/apple && xcodegen generate
	@echo "macOS ready! Open clients/apple/AxiomVault.xcodeproj and select AxiomVault-macOS scheme"

check-apple-deps:
	@if [ "$$(uname)" != "Darwin" ]; then \
		echo "ERROR: Apple development requires macOS"; \
		exit 1; \
	fi
	@echo "Checking Apple build dependencies..."
	@command -v xcodegen >/dev/null 2>&1 || { \
		echo ""; \
		echo "ERROR: xcodegen not found"; \
		echo "Install with: brew install xcodegen"; \
		echo ""; \
		exit 1; \
	}
	@command -v rustup >/dev/null 2>&1 || { \
		echo ""; \
		echo "ERROR: rustup not found"; \
		echo "Install from: https://rustup.rs"; \
		echo ""; \
		exit 1; \
	}
	@echo "✓ All Apple build dependencies found"
