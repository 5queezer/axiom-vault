# AxiomVault Build Helpers

.PHONY: all desktop desktop-release check-desktop-deps cli core ios ios-framework ios-project check-ios-deps help

# Default target
all: help

help:
	@echo "AxiomVault Build Targets"
	@echo "========================"
	@echo ""
	@echo "Desktop:"
	@echo "  make desktop         - Build desktop client (checks dependencies first)"
	@echo "  make desktop-release - Build desktop client in release mode"
	@echo "  make check-desktop-deps - Check desktop system dependencies"
	@echo ""
	@echo "iOS (macOS only):"
	@echo "  make ios             - Build complete iOS project (framework + Xcode project)"
	@echo "  make ios-framework   - Build Rust XCFramework for iOS"
	@echo "  make ios-project     - Generate Xcode project with XcodeGen"
	@echo "  make check-ios-deps  - Check iOS build dependencies"
	@echo ""
	@echo "Other:"
	@echo "  make cli             - Build CLI tool"
	@echo "  make core            - Build core libraries"
	@echo ""
	@echo "Direct cargo commands still work:"
	@echo "  cargo build --package axiomvault-desktop"
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
