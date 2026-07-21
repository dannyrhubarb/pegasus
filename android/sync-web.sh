#!/usr/bin/env bash
# Assemble the web build into android/app/src/main/assets/webroot/ — the
# Android app's bundled site. The Android twin of ios/sync-web.sh; both
# mirror .github/actions/build-site (keep all three in sync when the site's
# file set changes). Same deliberate differences as iOS: no version.json
# (the stale-cache toast is meaningless in-app), config.json fetched from
# the live deployment, and the injected revision suffixed -android.
#
# Run from anywhere; re-run after any game change, then rebuild the app.
set -euo pipefail
cd "$(dirname "$0")/.."

PAGES_URL="https://dannyrhubarb.github.io/pegasus"
DEST="android/app/src/main/assets/webroot"

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --release --target wasm32-unknown-unknown

rm -rf "$DEST"
mkdir -p "$DEST"
touch "$DEST/.gitkeep"

cp index.html manifest.json mq_js_bundle.js LICENSE third-party-licenses.html "$DEST/"
cp -R levels "$DEST/levels"

WASM_SRC="target/wasm32-unknown-unknown/release/pegasus.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz -o "$DEST/pegasus.wasm" "$WASM_SRC"
else
  echo "note: wasm-opt not found (apt/brew install binaryen) — bundling unoptimized wasm"
  cp "$WASM_SRC" "$DEST/pegasus.wasm"
fi

REV="$(git rev-parse --short=8 HEAD)-android"
BUILD_TIME="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
perl -pi -e "s/__GIT_REVISION__/${REV}/g; s/__BUILD_TIME__/${BUILD_TIME}/g" "$DEST/index.html"

python3 tools/gen-whats-new.py > "$DEST/whats-new.json" || {
  echo "note: gen-whats-new failed — the What's New screen will show its dev hint"
  rm -f "$DEST/whats-new.json"
}

if curl -fsS --max-time 10 "$PAGES_URL/config.json" -o "$DEST/config.json"; then
  echo "config.json fetched — online high scores enabled"
else
  rm -f "$DEST/config.json"
  echo "note: no config.json ($PAGES_URL unreachable or none deployed) — online scores off"
fi

echo "webroot ready: $(du -sh "$DEST" | cut -f1) at $DEST (revision ${REV})"
