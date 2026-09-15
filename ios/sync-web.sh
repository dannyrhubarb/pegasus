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

PAGES_URL="https://pegasusmoonlander.com"
# Old origin, kept as a fallback until the custom domain has soaked (#171).
# -L matters on it: once the custom domain is set, github.io 301-redirects.
PAGES_URL_LEGACY="https://dannyrhubarb.github.io/pegasus"
DEST="ios/Pegasus/WebRoot"

rm -rf "$DEST"
mkdir -p "$DEST"
touch "$DEST/.gitkeep"

cp index.html manifest.json mq_js_bundle.js LICENSE third-party-licenses.html privacy.html "$DEST/"
cp -R levels "$DEST/levels"
cp -R fonts "$DEST/fonts"

# The wasm, built the way the deploy builds it: pinned toolchain
# (rust-toolchain.toml), pinned wasm-opt, paths remapped — the same bytes
# the website ships for this commit (#214, reproducible builds).
tools/build-wasm.sh "$DEST/pegasus.wasm"

# Inject revision + build time like the deploy does (About screen; also the
# replay build id). The -ios suffix marks app-bundled builds apart in
# analytics/replays. perl, not sed -i: BSD sed on macOS needs -i ''.
REV="$(git rev-parse --short=8 HEAD)-ios"
BUILD_TIME="$(tools/version.sh --commit-date)" # the commit date, not the clock — reproducible
BUILD_VERSION="$(tools/version.sh)"
perl -pi -e "s/__GIT_REVISION__/${REV}/g; s/__BUILD_TIME__/${BUILD_TIME}/g; s/__BUILD_VERSION__/${BUILD_VERSION}/g" "$DEST/index.html"

# What's New changelog (needs full git history — fine on a normal clone).
python3 tools/gen-whats-new.py > "$DEST/whats-new.json" || {
  echo "note: gen-whats-new failed — the What's New screen will show its dev hint"
  rm -f "$DEST/whats-new.json"
}

# Backend endpoints from the live site → online boards/ghost/analytics in the
# app. Offline or pre-backend deploys: the app runs with online scores off.
if curl -fsS --max-time 10 "$PAGES_URL/config.json" -o "$DEST/config.json" ||
   curl -fsSL --max-time 10 "$PAGES_URL_LEGACY/config.json" -o "$DEST/config.json"; then
  echo "config.json fetched — online high scores enabled"
else
  rm -f "$DEST/config.json"
  echo "note: no config.json ($PAGES_URL unreachable or none deployed) — online scores off"
fi

echo "WebRoot ready: $(du -sh "$DEST" | cut -f1) at $DEST (revision ${REV})"
