#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="e-sh"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d '"' -f2)"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
mkdir -p "$DIST"

OS="$(uname -s)"
ARCH="$(uname -m)"

build_macos_universal() {
  echo ">>> macOS universal ($BIN_NAME $VERSION)"
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin

  local universal="$DIST/${BIN_NAME}-universal"
  mkdir -p "$DIST"
  lipo -create -output "$universal" \
    "target/aarch64-apple-darwin/release/$BIN_NAME" \
    "target/x86_64-apple-darwin/release/$BIN_NAME"

  local stage="$DIST/${BIN_NAME}-${VERSION}-macos-universal"
  rm -rf "$stage"
  mkdir -p "$stage"

  bash "$ROOT/scripts/make-app.sh" "$universal" "$stage" >/dev/null
  rm -f "$universal"
  cp README.md "$stage/" 2>/dev/null || true

  tar -C "$DIST" -czf "$stage.tar.gz" "$(basename "$stage")"
  rm -rf "$stage"
  ( cd "$DIST" && shasum -a 256 "$(basename "$stage").tar.gz" > "$(basename "$stage").tar.gz.sha256" )
  echo "    -> $stage.tar.gz (contains ${APP_NAME:-$BIN_NAME}.app)"
}

build_linux_x86_64() {
  echo ">>> Linux x86_64 ($BIN_NAME $VERSION)"
  rustup target add x86_64-unknown-linux-gnu >/dev/null
  cargo build --release --target x86_64-unknown-linux-gnu

  local stage="$DIST/${BIN_NAME}-${VERSION}-linux-x86_64"
  rm -rf "$stage"
  mkdir -p "$stage"
  cp "target/x86_64-unknown-linux-gnu/release/$BIN_NAME" "$stage/"
  cp README.md "$stage/" 2>/dev/null || true

  tar -C "$DIST" -czf "$stage.tar.gz" "$(basename "$stage")"
  rm -rf "$stage"
  ( cd "$DIST" && sha256sum "$(basename "$stage").tar.gz" > "$(basename "$stage").tar.gz.sha256" )
  echo "    -> $stage.tar.gz"
}

build_windows_x86_64() {
  echo ">>> Windows x86_64 ($BIN_NAME $VERSION)"
  rustup target add x86_64-pc-windows-msvc >/dev/null
  cargo build --release --target x86_64-pc-windows-msvc

  local stage="$DIST/${BIN_NAME}-${VERSION}-windows-x86_64"
  rm -rf "$stage"
  mkdir -p "$stage"
  cp "target/x86_64-pc-windows-msvc/release/${BIN_NAME}.exe" "$stage/"
  cp README.md "$stage/" 2>/dev/null || true

  ( cd "$DIST" && rm -f "$(basename "$stage").zip" && \
      powershell -NoProfile -Command "Compress-Archive -Path '$(basename "$stage")' -DestinationPath '$(basename "$stage").zip'" \
      || (command -v zip >/dev/null && zip -rq "$(basename "$stage").zip" "$(basename "$stage")") )
  rm -rf "$stage"
  ( cd "$DIST" && (sha256sum "$(basename "$stage").zip" 2>/dev/null || shasum -a 256 "$(basename "$stage").zip") > "$(basename "$stage").zip.sha256" )
  echo "    -> $stage.zip"
}

case "$OS" in
  Darwin)  build_macos_universal ;;
  Linux)   build_linux_x86_64 ;;
  MINGW*|MSYS*|CYGWIN*) build_windows_x86_64 ;;
  *)
    echo "unsupported host OS: $OS" >&2
    exit 1
    ;;
esac

echo
echo "Done. Artifacts in: $DIST"
ls -lh "$DIST"
