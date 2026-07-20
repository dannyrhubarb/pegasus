#!/usr/bin/env bash
# Assemble the web build into ios/Pegasus/WebRoot/ — the iOS app's bundled
# site. Mirrors .github/actions/build-site (keep the two in sync), minus the
# web-only bits: no version.json (the stale-cache reload toast is meaningless
# when the page ships inside the app binary; the page treats the 404 as
# "feature off") and config.json is pulled from the live deployment so the
# app gets online scores without needing the BACKEND_CONFIG_JSON secret.
#
# Run from anywhere; re-run after any game change, then build in Xcode (the
# WebRoot folder reference re-copies on every build).
set -euo pipefail
cd "$(dirname "$0")/.."

PAGES_URL="https://dannyrhubarb.github.io/pegasus"
DEST="ios/Pegasus/WebRoot"

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --release --target wasm32-unknown-unknown

rm -rf "$DEST"
mkdir -p "$DEST"
touch "$DEST/.gitkeep"

cp index.html manifest.json mq_js_bundle.js LICENSE third-party-licenses.html "$DEST/"
cp -R levels "$DEST/levels"

# wasm-opt (brew install binaryen) is optional locally — it only shrinks the
# binary, same as the deploy.
WASM_SRC="target/wasm32-unknown-unknown/release/pegasus.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz -o "$DEST/pegasus.wasm" "$WASM_SRC"
else
  echo "note: wasm-opt not found (brew install binaryen) — bundling unoptimized wasm"
  cp "$WASM_SRC" "$DEST/pegasus.wasm"
fi

# Inject revision + build time like the deploy does (About screen; also the
# replay build id). The -ios suffix marks app-bundled builds apart in
# analytics/replays. perl, not sed -i: BSD sed on macOS needs -i ''.
REV="$(git rev-parse --short=8 HEAD)-ios"
BUILD_TIME="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
perl -pi -e "s/__GIT_REVISION__/${REV}/g; s/__BUILD_TIME__/${BUILD_TIME}/g" "$DEST/index.html"

# What's New changelog (needs full git history — fine on a normal clone).
python3 tools/gen-whats-new.py > "$DEST/whats-new.json" || {
  echo "note: gen-whats-new failed — the What's New screen will show its dev hint"
  rm -f "$DEST/whats-new.json"
}

# Backend endpoints from the live site → online boards/ghost/analytics in the
# app. Offline or pre-backend deploys: the app runs with online scores off.
if curl -fsS --max-time 10 "$PAGES_URL/config.json" -o "$DEST/config.json"; then
  echo "config.json fetched — online high scores enabled"
else
  rm -f "$DEST/config.json"
  echo "note: no config.json ($PAGES_URL unreachable or none deployed) — online scores off"
fi

echo "WebRoot ready: $(du -sh "$DEST" | cut -f1) at $DEST (revision ${REV})"
