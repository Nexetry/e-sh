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
  ( cd "$DIST" && shasum -a 256 "$(basename "$stage").tar.gz" > "$(basename "$stage").tar.gz.sha256" )
  echo "    -> $stage.tar.gz (contains ${BIN_NAME}.app)"

  build_macos_dmg "$stage"
  rm -rf "$stage"
}

build_macos_dmg() {
  local stage="$1"
  local dmg_path="$DIST/${BIN_NAME}-${VERSION}-macos-universal.dmg"
  rm -f "$dmg_path"

  if command -v create-dmg >/dev/null 2>&1; then
    echo "    ... building .dmg via create-dmg"
    create-dmg \
      --volname "${BIN_NAME} ${VERSION}" \
      --window-pos 200 120 \
      --window-size 640 360 \
      --icon-size 96 \
      --icon "${BIN_NAME}.app" 160 180 \
      --hide-extension "${BIN_NAME}.app" \
      --app-drop-link 480 180 \
      --no-internet-enable \
      "$dmg_path" \
      "$stage/${BIN_NAME}.app" >/dev/null
  else
    echo "    ... create-dmg not found; falling back to hdiutil"
    local tmp_src
    tmp_src="$(mktemp -d)"
    cp -R "$stage/${BIN_NAME}.app" "$tmp_src/"
    ln -sf /Applications "$tmp_src/Applications"
    hdiutil create \
      -volname "${BIN_NAME} ${VERSION}" \
      -srcfolder "$tmp_src" \
      -ov -format UDZO \
      "$dmg_path" >/dev/null
    rm -rf "$tmp_src"
  fi

  ( cd "$DIST" && shasum -a 256 "$(basename "$dmg_path")" > "$(basename "$dmg_path").sha256" )
  echo "    -> $dmg_path"
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

  build_linux_deb
}

build_linux_deb() {
  if ! command -v cargo-deb >/dev/null 2>&1; then
    echo "    ... cargo-deb not installed; skipping .deb (install with: cargo install cargo-deb)"
    return 0
  fi
  echo "    ... building .deb via cargo-deb"
  local deb_out
  deb_out="$(cargo deb --no-build --target x86_64-unknown-linux-gnu --output "$DIST" 2>&1 | tee /dev/stderr | tail -1)"
  local deb_path
  deb_path="$(ls -1t "$DIST"/*.deb 2>/dev/null | head -1 || true)"
  if [[ -n "$deb_path" && -f "$deb_path" ]]; then
    ( cd "$DIST" && sha256sum "$(basename "$deb_path")" > "$(basename "$deb_path").sha256" )
    echo "    -> $deb_path"
  else
    echo "    !! cargo-deb did not produce a .deb: $deb_out" >&2
    return 1
  fi
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
