# AxiomVault Build & Install Helpers

# Installation variables (can be overridden)
PREFIX ?= /usr/local
DESTDIR ?= /
BINDIR = $(DESTDIR)$(PREFIX)/bin
DATADIR = $(DESTDIR)$(PREFIX)/share
DESKTOPDIR = $(DATADIR)/applications
ICONDIR = $(DATADIR)/icons/hicolor

# Application info
APP_NAME = axiomvault-desktop
APP_BINARY = target/release/$(APP_NAME)
APP_DESKTOP = clients/desktop/axiomvault.desktop
APP_ICON = clients/desktop/axiomvault.svg

.PHONY: all desktop desktop-release check-desktop-deps cli core install install-desktop uninstall uninstall-desktop clean-install help
.PHONY: ios ios-framework ios-project check-ios-deps

# Default target
all: help

help:
	@echo "AxiomVault Build & Install Targets"
	@echo "===================================="
	@echo ""
	@echo "Build Targets:"
	@echo "  make desktop         - Build desktop client (checks dependencies first)"
	@echo "  make desktop-release - Build desktop client in release mode"
	@echo "  make check-desktop-deps - Check desktop system dependencies"
	@echo "  make cli             - Build CLI tool"
	@echo "  make core            - Build core libraries"
	@echo ""
	@echo "Install Targets:"
	@echo "  make install         - Build and install desktop app system-wide"
	@echo "  make install-desktop - Install pre-built release binary"
	@echo "  make uninstall       - Remove installed desktop app"
	@echo ""
	@echo "Advanced Install:"
	@echo "  PREFIX=/opt/axiom make install  - Install to /opt/axiom"
	@echo "  DESTDIR=/staging make install   - Stage install to /staging"
	@echo ""
	@echo "Troubleshooting:"
	@echo "  sudo make uninstall && sudo make install  - Full reinstall"
	@echo "  sudo kbuildsycoca5 --noincremental      - Force KDE cache update"
	@echo ""
	@echo "iOS (macOS only):"
	@echo "  make ios             - Build complete iOS project"
	@echo "  make ios-framework   - Build Rust XCFramework for iOS"
	@echo "  make ios-project     - Generate Xcode project"
	@echo "  make check-ios-deps  - Check iOS build dependencies"
	@echo ""
	@echo "Cleanup:"
	@echo "  make clean-install   - Remove all installed files"
	@echo ""

# Desktop client with dependency check
desktop: check-desktop-deps
	cargo build --package axiomvault-desktop

desktop-release: check-desktop-deps
	cargo build --package axiomvault-desktop --release

# CLI tool
cli:
	cargo build --package axiomvault-cli

# Core libraries
core:
	cargo build --workspace --exclude axiomvault-desktop --exclude axiomvault-cli

# Install targets
install: desktop-release install-desktop
	@echo ""
	@echo "✅ AxiomVault installed successfully!"
	@echo "Launch with: axiomvault-desktop"
	@echo "Or find in your application menu"

install-desktop: $(APP_BINARY) | create-launcher create-desktop-entry create-icon update-cache
	@echo "Installing binary to $(BINDIR)..."
	@mkdir -p $(BINDIR)
	@install -m 755 $(APP_BINARY) $(BINDIR)/$(APP_NAME)
	@ln -sf $(BINDIR)/$(APP_NAME) $(BINDIR)/axiomvault 2>/dev/null || true
	@echo "✓ Binary installed"

create-launcher:
	@echo "Installing launcher wrapper..."
	@mkdir -p $(BINDIR)
	@install -m 755 clients/desktop/axiomvault-launcher.sh $(BINDIR)/axiomvault-launcher.sh
	@echo "✓ Launcher installed"

create-desktop-entry: $(APP_DESKTOP)
	@echo "Installing desktop entry..."
	@mkdir -p $(DESKTOPDIR)
	@install -m 644 $(APP_DESKTOP) $(DESKTOPDIR)/axiomvault.desktop
	@echo "✓ Desktop entry installed"

create-icon: $(APP_ICON)
	@echo "Installing icon..."
	@mkdir -p $(ICONDIR)/scalable/apps
	@install -m 644 $(APP_ICON) $(ICONDIR)/scalable/apps/axiomvault.svg
	@echo "✓ Icon installed"

update-cache:
	@echo "Updating application cache..."
	@if command -v kbuildsycoca5 >/dev/null 2>&1; then \
		kbuildsycoca5 2>/dev/null || true; \
		echo "✓ KDE cache updated"; \
	fi
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache $(ICONDIR) 2>/dev/null || true; \
		echo "✓ GTK icon cache updated"; \
	fi
	@if command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database $(DESKTOPDIR) 2>/dev/null || true; \
		echo "✓ Desktop database updated"; \
	fi

uninstall: uninstall-desktop
	@echo "✅ AxiomVault uninstalled"

uninstall-desktop:
	@echo "Removing installed files..."
	@rm -f $(BINDIR)/$(APP_NAME)
	@rm -f $(BINDIR)/axiomvault
	@rm -f $(BINDIR)/axiomvault-launcher.sh
	@rm -f $(DESKTOPDIR)/axiomvault.desktop
	@rm -f $(ICONDIR)/scalable/apps/axiomvault.svg
	@echo "✓ Files removed"
	@echo "Updating application cache..."
	@if command -v kbuildsycoca5 >/dev/null 2>&1; then \
		kbuildsycoca5 2>/dev/null || true; \
	fi
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache $(ICONDIR) 2>/dev/null || true; \
	fi

clean-install: uninstall
	@echo "Cleaned up all installation files"

# Check desktop dependencies (Linux only)
check-desktop-deps:
	@if [ "$$(uname)" = "Linux" ]; then \
		echo "Checking system dependencies for desktop build..."; \
		command -v pkg-config >/dev/null 2>&1 || { \
			echo ""; \
			echo "ERROR: pkg-config not found"; \
			echo "Install with: sudo apt-get install pkg-config"; \
			echo ""; \
			exit 1; \
		}; \
		pkg-config --exists gtk+-3.0 2>/dev/null || { \
			echo ""; \
			echo "ERROR: GTK 3 not found"; \
			echo "Install with: sudo apt-get install libgtk-3-dev"; \
			echo ""; \
			exit 1; \
		}; \
		pkg-config --exists webkit2gtk-4.1 2>/dev/null || { \
			echo ""; \
			echo "ERROR: WebKit2GTK 4.1 not found"; \
			echo "Install with: sudo apt-get install libwebkit2gtk-4.1-dev"; \
			echo ""; \
			exit 1; \
		}; \
		echo "✓ All required dependencies found"; \
	else \
		echo "Skipping dependency check (not Linux)"; \
	fi

# iOS targets (macOS only)
ios: ios-framework ios-project
	@echo "iOS project ready! Open clients/ios/AxiomVault.xcodeproj in Xcode"

ios-framework: check-ios-deps
	@if [ "$$(uname)" = "Darwin" ]; then \
		echo "Building Rust XCFramework for iOS..."; \
		cd clients/ios/Scripts && ./build-ios.sh; \
	else \
		echo "ERROR: iOS framework can only be built on macOS"; \
		exit 1; \
	fi

ios-project: check-ios-deps
	@if [ "$$(uname)" = "Darwin" ]; then \
		echo "Generating Xcode project with XcodeGen..."; \
		cd clients/ios && xcodegen generate; \
		echo "✓ Generated AxiomVault.xcodeproj"; \
	else \
		echo "ERROR: Xcode project can only be generated on macOS"; \
		exit 1; \
	fi

check-ios-deps:
	@if [ "$$(uname)" = "Darwin" ]; then \
		echo "Checking iOS build dependencies..."; \
		command -v xcodegen >/dev/null 2>&1 || { \
			echo ""; \
			echo "ERROR: xcodegen not found"; \
			echo "Install with: brew install xcodegen"; \
			echo ""; \
			exit 1; \
		}; \
		command -v rustup >/dev/null 2>&1 || { \
			echo ""; \
			echo "ERROR: rustup not found"; \
			echo "Install from: https://rustup.rs"; \
			echo ""; \
			exit 1; \
		}; \
		command -v xcrun >/dev/null 2>&1 || { \
			echo ""; \
			echo "ERROR: Xcode command line tools not found"; \
			echo "Install with: xcode-select --install"; \
			echo ""; \
			exit 1; \
		}; \
		echo "✓ All iOS build dependencies found"; \
	else \
		echo "ERROR: iOS development requires macOS"; \
		exit 1; \
	fi
