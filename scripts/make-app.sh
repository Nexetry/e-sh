#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="e-sh"
APP_NAME="e-sh"
BUNDLE_ID="com.nexetry.e-sh"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_BIN="${1:-}"
OUT_DIR="${2:-$ROOT/dist}"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d '"' -f2)"

if [[ -z "$SRC_BIN" || ! -f "$SRC_BIN" ]]; then
  echo "usage: $0 <path-to-binary> [out-dir]" >&2
  exit 1
fi

APP_DIR="$OUT_DIR/${APP_NAME}.app"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$SRC_BIN" "$APP_DIR/Contents/MacOS/$BIN_NAME"
chmod +x "$APP_DIR/Contents/MacOS/$BIN_NAME"

if [[ -f "$ROOT/assets/AppIcon.icns" ]]; then
  cp "$ROOT/assets/AppIcon.icns" "$APP_DIR/Contents/Resources/AppIcon.icns"
fi

sed "s/__VERSION__/$VERSION/g" "$ROOT/assets/Info.plist" > "$APP_DIR/Contents/Info.plist"

codesign --force --deep --sign - "$APP_DIR" >/dev/null 2>&1 || true

echo "$APP_DIR"
