#!/bin/bash
# Launcher script for AxiomVault Desktop
# Resolves GLib library conflicts with conda environments

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BINARY="$PROJECT_ROOT/target/debug/axiomvault-desktop"

# Check if binary exists
if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found at $BINARY"
    echo "Please build first with: cargo build -p axiomvault-desktop"
    exit 1
fi

# Detect if conda is active and might cause library conflicts
if [ -n "$CONDA_PREFIX" ] || [ -n "$CONDA_DEFAULT_ENV" ]; then
    echo "Note: Conda environment detected. Adjusting library paths to avoid conflicts..."

    # Prioritize system libraries over conda libraries
    # This fixes the g_once_init_leave_pointer symbol issue
    if [ -d "/usr/lib64" ]; then
        export LD_LIBRARY_PATH="/usr/lib64:/usr/lib:${LD_LIBRARY_PATH}"
    elif [ -d "/usr/lib/x86_64-linux-gnu" ]; then
        export LD_LIBRARY_PATH="/usr/lib/x86_64-linux-gnu:/usr/lib:${LD_LIBRARY_PATH}"
    fi
fi

# Run the application
exec "$BINARY" "$@"
