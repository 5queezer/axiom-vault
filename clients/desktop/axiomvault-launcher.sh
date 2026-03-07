#!/bin/bash
# AxiomVault launcher - handles display and rendering compatibility

# Deactivate conda if active (interferes with GTK)
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

# === Display Server Configuration ===
# Force X11 backend (more stable than Wayland for WebKit)
export GDK_BACKEND=x11

# Disable Wayland
unset WAYLAND_DISPLAY
unset WAYLAND_SOCKET

# Ensure DISPLAY is set
if [ -z "$DISPLAY" ]; then
    export DISPLAY=:0
fi

# === WebKit/GTK Rendering Configuration ===
# CRITICAL: Disable GPU acceleration (prevents "Failed to create GBM buffer" errors)
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export COGL_DRIVER=gl
export COGL_DISABLE_GL_EXTENSIONS=OES_EGL_image_external

# Disable hardware acceleration in WebKit
export WEBKIT_USE_SANDBOX=0

# Use software rendering if needed
export LIBGL_ALWAYS_INDIRECT=1

# === GTK Configuration ===
# Don't set GTK_DEBUG (causes warnings)
unset GTK_DEBUG

# Use Adwaita theme
export GTK_THEME=Adwaita:light
export GTK_CSD=1

# Set session type
export XDG_SESSION_TYPE=x11

# === Application Launch ===
exec "$BINARY" "$@"
