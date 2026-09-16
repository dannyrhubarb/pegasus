#!/usr/bin/env bash
# Assemble the web build into android/app/src/main/assets/webroot/ — the
# Android app's bundled site. The Android twin of ios/sync-web.sh; both
# mirror .github/actions/build-site (keep all three in sync when the site's
# file set changes). Same deliberate differences as iOS: no version.json
# (the stale-cache toast is meaningless in-app), config.json from
# PEGASUS_BACKEND_CONFIG (CI) with the live deployment as the local
# fallback. The injected revision is the plain sha (no platform suffix —
# the About screen and the analytics device-mix already name the shell).
#
# Run from anywhere; re-run after any game change, then rebuild the app.
set -euo pipefail
cd "$(dirname "$0")/.."

PAGES_URL="https://pegasusmoonlander.com"
# Old origin, kept as a fallback until the custom domain has soaked (#171).
# -L matters on it: once the custom domain is set, github.io 301-redirects.
PAGES_URL_LEGACY="https://dannyrhubarb.github.io/pegasus"
DEST="android/app/src/main/assets/webroot"

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

# PEGASUS_REV lets CI name the build itself — the on-demand PR test APK
# passes the PR HEAD sha, since the merge ref this checkout sits on has a
# merge-commit sha that means nothing to a tester reading the About screen.
REV="${PEGASUS_REV:-$(git rev-parse --short=8 HEAD)}"
BUILD_VERSION="$(tools/version.sh)"
perl -pi -e "s/__GIT_REVISION__/${REV}/g; s/__BUILD_VERSION__/${BUILD_VERSION}/g" "$DEST/index.html"

python3 tools/gen-whats-new.py > "$DEST/whats-new.json" || {
  echo "note: gen-whats-new failed — the What's New screen will show its dev hint"
  rm -f "$DEST/whats-new.json"
}

# Backend endpoints (online boards / ghost / analytics in the app). CI
# passes the BACKEND_CONFIG_JSON repo variable as PEGASUS_BACKEND_CONFIG —
# the same JSON the web deploy writes, validated the same way — so the
# bundle is a function of (commit, config) and two builds of one commit
# match; fetching whatever the live site served at build time was the one
# input the Android reproducibility check could not pin (#214 step 6).
# Locally the variable is not available, so an unset PEGASUS_BACKEND_CONFIG
# falls back to the live deployment (offline ⇒ online scores off). To
# reproduce a CI build, pass the config.json it shipped.
if [ -n "${PEGASUS_BACKEND_CONFIG:-}" ]; then
  printf '%s' "$PEGASUS_BACKEND_CONFIG" | python3 -c 'import json,sys; c = json.load(sys.stdin); assert c["apiBaseUrl"].startswith("https://") and c["replayBaseUrl"].startswith("https://")'
  printf '%s\n' "$PEGASUS_BACKEND_CONFIG" > "$DEST/config.json"
  echo "config.json from PEGASUS_BACKEND_CONFIG — online high scores enabled"
elif curl -fsS --max-time 10 "$PAGES_URL/config.json" -o "$DEST/config.json" ||
     curl -fsSL --max-time 10 "$PAGES_URL_LEGACY/config.json" -o "$DEST/config.json"; then
  echo "config.json fetched from the live site (PEGASUS_BACKEND_CONFIG unset) — online high scores enabled"
else
  rm -f "$DEST/config.json"
  echo "note: no config.json ($PAGES_URL unreachable or none deployed) — online scores off"
fi

echo "webroot ready: $(du -sh "$DEST" | cut -f1) at $DEST (revision ${REV})"
