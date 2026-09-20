#!/usr/bin/env bash
# Assemble the deployable website into site/ — the ONE build recipe behind
# the main deploy, the per-PR previews and ci.yml's reproducibility check
# (which runs it twice and diffs the result). Both app shells mirror the
# file list in their sync-web.sh (tools/check-bundle-sync.py pins the three
# lists to each other).
#
#   GIT_REV=<label> [BACKEND_CONFIG=<json>] [APPLE_TEAM_ID=<id>] tools/build-site.sh
#
# Everything here is a pure function of the checked-out commit plus the two
# optional deploy inputs (the backend config and the Apple Team ID, which
# come from repository settings, not from git). See CLAUDE.md "Reproducible
# builds".
set -euo pipefail
cd "$(dirname "$0")/.."
: "${GIT_REV:?set GIT_REV to the revision label (e.g. the short sha)}"

# Layout (2026-09, the landing page): the site ROOT is the marketing page
# (landing.html → site/index.html — store links, a Play button) and THE
# GAME LIVES UNDER site/play/ (index.html + everything it fetches; every
# asset URL in it is relative, so it is location-agnostic — the shells
# still bundle it at their own root, previews serve it at pr-<n>/play/).
# What stays at the root is exactly what something OUTSIDE the page reaches
# by absolute URL: app-policy.json (baked into every installed app),
# .well-known/ (both OSes read it at the root), privacy.html (the store
# listings' privacy-policy URL). See CLAUDE.md "Landing page".
rm -rf site
mkdir -p site/play
cp landing.html site/index.html
# The official App Store / Google Play badge artwork the landing page
# shows (trademark art used per Apple's and Google's badge guidelines —
# scaled only, never recoloured or redrawn). Web-only: a shell IS the app.
cp -r badges site/
cp index.html site/play/
cp editor.html site/play/
cp manifest.json site/play/
# The web icons (apple-touch-icon + the PWA manifest's) are COMMITTED
# renders of icon.svg, not generated here: rendering at deploy time pulled
# in whatever librsvg the runner image carried, which is one more input a
# reproducible build cannot pin. Re-render when icon.svg changes:
#   for s in 512 192 180; do rsvg-convert -w $s -h $s icon.svg -o icon-$s.png; done
cp icon-512.png icon-192.png icon-180.png site/play/
cp mq_js_bundle.js site/play/
# License compliance: the GPL text + the generated third-party attribution
# page (linked from the About screen) ship with the site.
cp LICENSE site/play/
cp third-party-licenses.html site/play/
# Standalone privacy policy (linked from the Play Store listing; same
# content as the About screen's privacy note). At the ROOT because the
# store listings point at https://pegasusmoonlander.com/privacy.html, and
# ALSO next to the game so the bundle lists stay one file set (the shells
# copy it beside their index.html).
cp privacy.html site/
cp privacy.html site/play/
# Levels are runtime data (fetched by index.html) — new levels ship by
# editing levels/ + manifest.json, no wasm rebuild needed.
cp -r levels site/play/
# Vendored menu font (JetBrains Mono woff2 + its OFL license text),
# referenced by index.html/editor.html @font-face.
cp -r fonts site/play/
# Digital Asset Links (Android App Links / related-app detection): a
# dot-dir, so it rides cp -a through sync-pages-branch and the Pages
# artifact; .nojekyll keeps Pages from dropping it. Web-only (nothing in
# an app shell fetches it — see check-bundle-sync.py).
cp -r .well-known site/
# apple-app-site-association (Universal Links): the repo copy carries an
# __APPLE_TEAM_ID__ placeholder so the Team ID stays a secret rather than
# repo content (owner preference — the file is public once served, but
# forks shouldn't inherit it). No Team ID = no AASA.
if [ -n "${APPLE_TEAM_ID:-}" ]; then
  perl -pi -e "s/__APPLE_TEAM_ID__/${APPLE_TEAM_ID}/g" site/.well-known/apple-app-site-association
else
  rm site/.well-known/apple-app-site-association
fi

# The wasm: pinned toolchain + pinned wasm-opt, paths remapped (see the
# script). Prints the sha256 — compare it against a local build.
tools/build-wasm.sh site/play/pegasus.wasm

# Inject the build's identity into the info overlay: the git revision and
# the tag-derived VERSION (tools/version.sh — `1.3.0+14`, see CLAUDE.md
# "Versioning"). Both are pure functions of the checked-out commit — the
# wall-clock build time that once rode along here was the one input that
# could never reproduce (#214), and the About row that showed it is gone.
# The landing page carries the REVISION only: its analytics tags every
# event with it (the `build` field), and an un-stamped `__GIT_REVISION__`
# is how a local checkout recognises itself and stays silent.
BUILD_VERSION=$(tools/version.sh)
perl -pi -e "s/__GIT_REVISION__/${GIT_REV}/g; s/__BUILD_VERSION__/${BUILD_VERSION}/g" site/play/index.html site/index.html
# Version marker fetched with cache bypassed by index.html, to detect a
# stale cached page and offer the "new build" reload toast.
printf '{"revision":"%s","version":"%s"}\n' "${GIT_REV}" "${BUILD_VERSION}" > site/play/version.json
# What's New data for the About screen: curated backfill + every commit
# carrying a `Whats-new:` trailer. Needs full git history — the workflows
# check out with fetch-depth: 0 (a shallow clone silently drops every
# trailered commit below HEAD).
python3 tools/gen-whats-new.py > site/play/whats-new.json
# Backend endpoints (online high scores). Comes from the BACKEND_CONFIG_JSON
# repo variable so URLs rotate without a code change; validated so a
# malformed variable fails the build here rather than silently shipping a
# broken config.json. Absent = no config.json = the game runs offline-only.
if [ -n "${BACKEND_CONFIG:-}" ]; then
  printf '%s' "${BACKEND_CONFIG}" | python3 -c 'import json,sys; c = json.load(sys.stdin); assert c["apiBaseUrl"].startswith("https://") and c["replayBaseUrl"].startswith("https://")'
  printf '%s\n' "${BACKEND_CONFIG}" > site/play/config.json
fi
# App update policy (forced-update wall + runtime config override, #190):
# a CHECKED-IN file — policy changes are commits (reviewed, diffable,
# self-deploying on the push to main), never console state. Validated so
# a malformed edit fails every PR's preview deploy rather than shipping a
# file every client parses; all keys optional ({} = no verdicts,
# everything fails open), but what's present must be well-shaped. Clients
# FETCH it live — the shells never bundle it (see WEB_ONLY in
# tools/check-bundle-sync.py). Two copies: the ROOT one is what every
# installed app fetches by absolute URL (that path is baked into the
# shells and can never move), the play/ one is what the web page fetches
# RELATIVE to itself (it stays location-agnostic). Same bytes.
python3 -c 'import json,sys,re; p=json.load(sys.stdin); assert isinstance(p,dict); assert all(all(k in ("android","ios") and isinstance(v,int) for k,v in p.get(b,{}).items()) for b in ("minBuild","recommendBuild")); assert all(p.get(t) is None or re.fullmatch(r"\d+\.\d+\.\d+(\+\d+)?", p[t]) for t in ("minVersion","recommendVersion")); assert not {"minWebBuildTime","recommendWebBuildTime"} & p.keys(), "retired: use minVersion/recommendVersion"; assert isinstance(p.get("message",""),str) and isinstance(p.get("recommendMessage",""),str); assert all(k in ("android","ios") and isinstance(v,str) for k,v in p.get("storeUrls",{}).items()); c=p.get("config"); assert c is None or (c["apiBaseUrl"].startswith("https://") and c["replayBaseUrl"].startswith("https://"))' < app-policy.json
cp app-policy.json site/
cp app-policy.json site/play/

echo "site/ ready: $(du -sh site | cut -f1), revision ${GIT_REV}, version ${BUILD_VERSION}"
