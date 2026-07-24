# Pegasus iOS app

A thin native shell that bundles the web build (wasm + `index.html` +
levels) into an offline-capable iOS app. The game runs unmodified in a
full-screen `WKWebView` — same WebKit engine as iOS Safari, so the touch
stick, safe-area handling, audio unlock and menu all behave exactly like
the website. Bundled files are served through a custom `pegasus://` URL
scheme (`WebRootSchemeHandler`) because `fetch()` — which the page uses
for the wasm, level files, manifest and config — does not work on
`file://` URLs. Online high scores, the ghost and analytics still work
over the network when the bundled `config.json` is present.

## Prerequisites

- A Mac with Xcode 15 or newer
- The Rust toolchain (`rustup`); the sync script adds the
  `wasm32-unknown-unknown` target itself
- Optional: `brew install binaryen` for `wasm-opt` (smaller wasm, like the
  deploy), `python3` for the What's New page (preinstalled on macOS)
- An iPhone/iPad + any Apple ID (free) for on-device signing, or an Apple
  Developer Program membership for TestFlight/App Store

## Build & run

```bash
./ios/sync-web.sh        # build the wasm and assemble ios/Pegasus/WebRoot/
open ios/Pegasus.xcodeproj
```

In Xcode:
1. Select the **Pegasus** target → Signing & Capabilities → pick your
   **Team** (add your Apple ID under Settings → Accounts first). Leave the
   bundle id `se.danielfalk.pegasus` (a free Personal Team may require
   making it unique — any suffix works).
2. Plug in your device, select it as the run destination, press **Run**.
3. First launch on a free team: the device blocks the app until you trust
   your developer profile under **Settings → General → VPN & Device
   Management**.

Free-account signing expires after **7 days** — reconnect and Run again to
re-sign. A paid Developer Program membership extends that to a year and
unlocks TestFlight/App Store distribution.

After changing the game, re-run `./ios/sync-web.sh` and build again — the
`WebRoot` folder reference re-copies into the app on every build, no Xcode
project changes needed.

## What the sync script bundles (vs. the web deploy)

`ios/sync-web.sh` mirrors `.github/actions/build-site` with three
deliberate differences:

- **No `version.json`** — the stale-cache reload toast makes no sense when
  the page ships inside the app binary; app updates replace the whole
  bundle. The page treats the 404 as "feature off".
- **`config.json` is fetched from the live GitHub Pages deployment** (the
  repo variable that writes it at deploy time isn't available locally).
  Offline build machine ⇒ the app runs with online scores disabled.
- **The injected revision carries an `-ios` suffix** so app builds are
  distinguishable in the About screen, analytics and replay build ids.

## Gotchas

- **WebRoot/ is gitignored** (like `pegasus.wasm`) — it's a build product.
  The `.gitkeep` keeps the folder present for the Xcode folder reference.
- The scheme handler strips query strings (`?v=`, `?fresh=`) and answers
  404 for missing optional files — both are load-bearing, don't "fix" them.
- The webview must fill the **whole screen** (not the safe area): the page
  uses `viewport-fit=cover` and reads `env(safe-area-inset-*)` itself.
- `allowsBackForwardNavigationGestures` stays **on**: the game mirrors its
  screen stack into session history, so the iOS edge-swipe means "back one
  screen in the game UI", same as in Safari.
- The app icon (`Assets.xcassets/AppIcon.appiconset/AppIcon1024.png`) is
  rendered from the repo's `icon.svg`. If the icon changes, re-render at
  1024×1024 (any SVG rasterizer; the icon has its own opaque background).

## CI (GitHub Actions)

Two workflows, both on free public-repo macOS runners:

- **`ios-build.yml`** — on every PR touching `ios/`: runs `sync-web.sh`
  and an **unsigned** `xcodebuild` so shell/project regressions are caught
  before anyone opens Xcode. Needs no secrets.
- **`ios-testflight.yml`** — on manual dispatch (Actions → iOS TestFlight
  → Run workflow) and on pushes to `main` touching `ios/`: archives with
  xcodebuild **cloud signing** (the App Store Connect API key creates and
  fetches the distribution certificate + profile on the fly — nothing
  signing-related lives in the repo) and uploads to TestFlight.
  `CFBundleVersion` = the workflow run number, so every upload is a new
  build. Requires four repository secrets:
  `APP_STORE_CONNECT_API_KEY_ID`, `APP_STORE_CONNECT_API_ISSUER_ID`,
  `APP_STORE_CONNECT_API_KEY_P8` (the whole `.p8` file), and
  `APPLE_TEAM_ID` — plus an existing App Store Connect app record for the
  bundle id. `Info.plist` sets `ITSAppUsesNonExemptEncryption = false`
  (HTTPS only = exempt) so uploads skip the per-build compliance question.
  After the upload, `ios/testflight-distribute.py` (App Store Connect
  API, same key) waits for Apple's build processing, submits the build to
  Beta App Review, and attaches it to the beta group named by the
  `TESTFLIGHT_GROUP_NAME` repo variable (default **"Public beta"**) — so
  external testers and the public link get every build with zero console
  clicks. No group with that name yet = soft no-op. The wait adds
  ~5–15 min of runner time to the job.

The committed **shared scheme**
(`Pegasus.xcodeproj/xcshareddata/xcschemes/Pegasus.xcscheme`) is what lets
`xcodebuild -scheme Pegasus` work on a fresh runner — Xcode only
auto-generates schemes locally. Keep it checked in.

The TestFlight pipeline was verified end-to-end 2026-07-20 (build 5, the
first accepted upload). Re-checked 2026-07-24 after Apple approved the
Beta App Review — this run exercises the full hands-free path incl. the
automatic beta-group assignment.
