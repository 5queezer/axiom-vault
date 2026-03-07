#!/bin/bash
# AxiomVault launcher wrapper - handles display server compatibility

# Deactivate conda if active (can interfere with display)
if [ -n "$CONDA_PREFIX" ]; then
    eval "$(conda shell.bash hook)" 2>/dev/null
    conda deactivate 2>/dev/null || true
fi

# Find the binary
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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

# GTK/WebKit backend preferences for compatibility
# Use X11 backend if available (more stable for WebKit in virtual environments)
export GDK_BACKEND=x11

# Force X11 over Wayland for QT apps
export QT_QPA_PLATFORM=xcb

# GTK settings for WebKit
export GTK_DEBUG=
export GTK_CSD=1

# Disable Wayland
unset WAYLAND_DISPLAY
unset WAYLAND_SOCKET

# Ensure DISPLAY is set
if [ -z "$DISPLAY" ]; then
    export DISPLAY=:0
fi

# Prevent GTK warnings in headless/virtual environments
export GTK_THEME=Adwaita
export XDG_SESSION_TYPE=x11

# Run the application
exec "$BINARY" "$@"
