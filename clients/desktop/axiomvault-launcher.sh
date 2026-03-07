#!/bin/bash
# AxiomVault launcher wrapper - handles display server compatibility

set -e

# Resolve script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Try to find the binary
if [ -x "$SCRIPT_DIR/../../target/release/axiomvault-desktop" ]; then
    BINARY="$SCRIPT_DIR/../../target/release/axiomvault-desktop"
elif [ -x "/usr/local/bin/axiomvault-desktop" ]; then
    BINARY="/usr/local/bin/axiomvault-desktop"
elif [ -x "/usr/bin/axiomvault-desktop" ]; then
    BINARY="/usr/bin/axiomvault-desktop"
else
    echo "Error: axiomvault-desktop binary not found" >&2
    exit 1
fi

# Deactivate conda if active (can interfere with display)
if [ -n "$CONDA_PREFIX" ]; then
    echo "Deactivating conda environment..."
    eval "$(conda shell.bash hook)"
    conda deactivate 2>/dev/null || true
fi

# Try to detect and use appropriate display server
if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ]; then
    # No display found, try to use X11
    export DISPLAY=:0
fi

# Prefer X11 if Wayland is causing issues
# Comment out the line below if you prefer Wayland
if command -v xcb-xwayland >/dev/null 2>&1 || [ "$XDG_SESSION_TYPE" = "x11" ]; then
    export QT_QPA_PLATFORM=xcb 2>/dev/null || true
fi

# Run the application
exec "$BINARY" "$@"
