#!/usr/bin/env bash
set -euo pipefail

# Build Clippi .dmg for macOS
# Usage: ./scripts/build-dmg.sh [release|debug]

BUILD_MODE="${1:-release}"
APP_NAME="Clippi"
TARGET="aarch64-apple-darwin"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ "$BUILD_MODE" = "release" ]; then
    BUNDLE_DIR="$PROJECT_DIR/target/$TARGET/release/bundle/osx"
    DMG_DIR="$PROJECT_DIR/target/$TARGET/release"
else
    BUNDLE_DIR="$PROJECT_DIR/target/$TARGET/debug/bundle/osx"
    DMG_DIR="$PROJECT_DIR/target/$TARGET/debug"
fi

echo "=== Building $APP_NAME ($BUILD_MODE) ==="

cd "$PROJECT_DIR"

# Install cargo-bundle if not present
if ! command -v cargo-bundle &> /dev/null; then
    echo "--- Installing cargo-bundle ---"
    cargo install cargo-bundle
fi

# Create .app bundle (cargo-bundle builds the binary too)
echo "--- Creating .app bundle ---"
if [ "$BUILD_MODE" = "release" ]; then
    cargo bundle --release --target "$TARGET"
else
    cargo bundle --target "$TARGET"
fi

APP_PATH="$BUNDLE_DIR/$APP_NAME.app"
if [ ! -d "$APP_PATH" ]; then
    echo "ERROR: .app bundle not found at $APP_PATH"
    exit 1
fi

echo "--- .app bundle created at $APP_PATH ---"

# Create .dmg
DMG_NAME="${APP_NAME}_aarch64.dmg"
DMG_PATH="$DMG_DIR/$DMG_NAME"

echo "--- Creating .dmg ---"
rm -f "$DMG_PATH"

hdiutil create -volname "$APP_NAME" \
    -srcfolder "$APP_PATH" \
    -ov -format UDZO \
    "$DMG_PATH"

echo ""
echo "=== Build complete ==="
echo "  App: $APP_PATH"
echo "  DMG: $DMG_PATH"
echo ""
ls -lh "$DMG_PATH"
