#!/usr/bin/env bash
# Validates that version strings across the repository are consistent.
# The canonical source of truth is the workspace version in Cargo.toml.
#
# Usage: scripts/check-versions.sh [expected-version]
# If expected-version is omitted, the Cargo.toml workspace version is used.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Extract canonical version from Cargo.toml workspace
CARGO_VERSION=$(sed -n '/\[workspace\.package\]/,/^\[/{ s/^version *= *"\(.*\)"/\1/p; }' "$REPO_ROOT/Cargo.toml")

EXPECTED="${1:-$CARGO_VERSION}"

if [ -z "$EXPECTED" ]; then
    echo "ERROR: Could not determine expected version"
    exit 1
fi

echo "Checking version consistency (expected: $EXPECTED)"
echo "---"

ERRORS=0

check() {
    local file="$1"
    local label="$2"
    local actual="$3"

    if [ "$actual" = "$EXPECTED" ]; then
        echo "  OK  $label ($file)"
    else
        echo "  FAIL  $label: got '$actual', expected '$EXPECTED' ($file)"
        ERRORS=$((ERRORS + 1))
    fi
}

# Cargo.toml workspace version
check "Cargo.toml" "workspace version" "$CARGO_VERSION"

# Apple project.yml MARKETING_VERSION
APPLE_MARKETING=$(sed -n 's/^    MARKETING_VERSION: *"\(.*\)"/\1/p' "$REPO_ROOT/clients/apple/project.yml")
check "clients/apple/project.yml" "MARKETING_VERSION" "$APPLE_MARKETING"

# Android versionName
ANDROID_VERSION=$(sed -n 's/.*versionName *= *"\(.*\)"/\1/p' "$REPO_ROOT/clients/android/app/build.gradle.kts")
check "clients/android/app/build.gradle.kts" "versionName" "$ANDROID_VERSION"

# Tauri version
TAURI_VERSION=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['version'])" "$REPO_ROOT/clients/desktop/src-tauri/tauri.conf.json")
check "clients/desktop/src-tauri/tauri.conf.json" "version" "$TAURI_VERSION"

echo "---"
if [ "$ERRORS" -gt 0 ]; then
    echo "FAILED: $ERRORS version(s) out of sync"
    exit 1
else
    echo "All versions consistent."
fi
