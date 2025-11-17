# AxiomVault Build Helpers

.PHONY: all desktop desktop-release check-desktop-deps cli core help

# Default target
all: help

help:
	@echo "AxiomVault Build Targets"
	@echo "========================"
	@echo ""
	@echo "  make desktop         - Build desktop client (checks dependencies first)"
	@echo "  make desktop-release - Build desktop client in release mode"
	@echo "  make cli             - Build CLI tool"
	@echo "  make core            - Build core libraries"
	@echo "  make check-desktop-deps - Check desktop system dependencies"
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
