#!/usr/bin/env bash
#
# AxiomVault Computer Use Test Runner
#
# Launches the desktop app (optionally under Xvfb) and runs the
# computer-use test harness against it using the Claude CLI.
#
# Usage:
#   ./run.sh                              # headless smoke test
#   ./run.sh --scenario create_vault      # headless, named scenario
#   ./run.sh --native "describe the UI"   # on your real display
#   ./run.sh --list-scenarios             # list available scenarios
#
# Environment variables:
#   AXIOM_CU_MAX_TURNS  - max conversation turns (default: 50)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DESKTOP_DIR="$PROJECT_ROOT/clients/desktop"
BINARY="$PROJECT_ROOT/target/debug/axiomvault-desktop"

XVFB_DISPLAY=":99"
XVFB_PID=""
APP_PID=""
PYTHON=""
NATIVE_MODE=false

# --- Cleanup ---
cleanup() {
    if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
        echo "Stopping AxiomVault (PID $APP_PID)..."
        kill "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    if [ -n "$XVFB_PID" ] && kill -0 "$XVFB_PID" 2>/dev/null; then
        echo "Stopping Xvfb (PID $XVFB_PID)..."
        kill "$XVFB_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- Find a usable Python ---
find_python() {
    for py in python3 python "$HOME/anaconda3/bin/python3" "$HOME/miniconda3/bin/python3"; do
        if command -v "$py" >/dev/null 2>&1; then
            PYTHON="$py"
            return 0
        fi
    done
    return 1
}

# --- Dependency checks ---
check_deps() {
    local missing=()

    command -v claude  >/dev/null 2>&1 || missing+=("claude (npm install -g @anthropic-ai/claude-code)")
    command -v xdotool >/dev/null 2>&1 || missing+=("xdotool")
    command -v scrot   >/dev/null 2>&1 || missing+=("scrot")

    if ! $NATIVE_MODE; then
        command -v Xvfb >/dev/null 2>&1 || missing+=("Xvfb")
    fi

    if ! find_python; then
        missing+=("python3")
    fi

    if [ ${#missing[@]} -gt 0 ]; then
        echo "Missing dependencies:"
        for dep in "${missing[@]}"; do
            echo "  - $dep"
        done
        echo ""
        echo "Install on Fedora:  sudo dnf install xdotool scrot xorg-x11-server-Xvfb"
        echo "Install on Ubuntu:  sudo apt install xdotool scrot xvfb"
        exit 1
    fi

    echo "Using Python: $PYTHON"
}

# --- Build app if needed ---
build_app() {
    if [ ! -f "$BINARY" ]; then
        echo "Building AxiomVault desktop..."
        cargo build -p axiomvault-desktop --manifest-path "$PROJECT_ROOT/Cargo.toml"
    fi
}

# --- Start Xvfb ---
start_xvfb() {
    echo "Starting Xvfb on display $XVFB_DISPLAY (${1}x${2})..."
    Xvfb "$XVFB_DISPLAY" -screen 0 "${1}x${2}x24" 2>/dev/null &
    XVFB_PID=$!
    sleep 1

    if ! kill -0 "$XVFB_PID" 2>/dev/null; then
        echo "Error: Xvfb failed to start"
        exit 1
    fi

    export DISPLAY="$XVFB_DISPLAY"

    # Force GTK/WebKit to use X11 instead of Wayland
    export GDK_BACKEND=x11
    export XDG_SESSION_TYPE=x11
    unset WAYLAND_DISPLAY

    # Prevent blank WebView in software-rendered Xvfb
    export WEBKIT_DISABLE_COMPOSITING_MODE=1
    export WEBKIT_DISABLE_DMABUF_RENDERER=1
}

# --- Launch AxiomVault ---
launch_app() {
    echo "Launching AxiomVault..."

    # Handle conda library conflicts (same as run-desktop.sh)
    if [ -n "${CONDA_PREFIX:-}" ] || [ -n "${CONDA_DEFAULT_ENV:-}" ]; then
        if [ -d "/usr/lib64" ]; then
            export LD_LIBRARY_PATH="/usr/lib64:/usr/lib:${LD_LIBRARY_PATH:-}"
        elif [ -d "/usr/lib/x86_64-linux-gnu" ]; then
            export LD_LIBRARY_PATH="/usr/lib/x86_64-linux-gnu:/usr/lib:${LD_LIBRARY_PATH:-}"
        fi
    fi

    "$BINARY" &
    APP_PID=$!

    # Wait for the window to appear
    echo "Waiting for app window..."
    local retries=0
    while ! xdotool search --name "AxiomVault" >/dev/null 2>&1; do
        sleep 1
        retries=$((retries + 1))
        if [ $retries -ge 15 ]; then
            echo "Error: App window did not appear within 15 seconds"
            exit 1
        fi
    done
    echo "App window detected."

    # Focus and resize the window to match expected dimensions
    local wid
    wid=$(xdotool search --name "AxiomVault" | head -1)
    xdotool windowactivate "$wid" 2>/dev/null || true
    xdotool windowsize "$wid" 1024 768 2>/dev/null || true
    xdotool windowmove "$wid" 0 0 2>/dev/null || true
    sleep 2
}

# --- Main ---
main() {
    local harness_args=()
    local width=1024
    local height=768

    # Parse our flags, pass the rest to harness.py
    while [[ $# -gt 0 ]]; do
        case $1 in
            --native)
                NATIVE_MODE=true
                shift
                ;;
            --list-scenarios|-l)
                check_deps
                "$PYTHON" "$SCRIPT_DIR/harness.py" --list-scenarios
                exit 0
                ;;
            *)
                harness_args+=("$1")
                shift
                ;;
        esac
    done

    # Default to smoke test if no args
    if [ ${#harness_args[@]} -eq 0 ]; then
        harness_args=("--scenario" "smoke_test")
    fi

    check_deps
    build_app

    if ! $NATIVE_MODE; then
        start_xvfb "$width" "$height"
    fi

    launch_app

    echo ""
    echo "=== Running test harness ==="
    echo ""
    "$PYTHON" "$SCRIPT_DIR/harness.py" "${harness_args[@]}"
}

main "$@"
