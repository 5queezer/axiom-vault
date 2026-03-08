#!/bin/bash
# AxiomVault launcher - handles display and rendering compatibility

# Remove conda library paths (conda's libgcc_s conflicts with system libs)
if [ -n "$CONDA_PREFIX" ]; then
    LD_LIBRARY_PATH=$(echo "$LD_LIBRARY_PATH" | tr ':' '\n' | grep -v conda | tr '\n' ':' | sed 's/:$//')
    export LD_LIBRARY_PATH
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
if [ -n "$WAYLAND_DISPLAY" ]; then
    # Wayland session - disable DMA-BUF renderer in WebKit2GTK
    # (fixes protocol errors with NVIDIA proprietary drivers)
    export WEBKIT_DISABLE_DMABUF_RENDERER=1
else
    # X11 session - ensure DISPLAY is set
    if [ -z "$DISPLAY" ]; then
        export DISPLAY=:0
    fi
fi

# === GTK Configuration ===
unset GTK_DEBUG

# Use Adwaita theme (dark variant to match the app's dark UI)
export GTK_THEME=Adwaita:dark

# === Application Launch ===
exec "$BINARY" "$@"
