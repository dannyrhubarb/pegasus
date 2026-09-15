# App Store assets

Everything App Store Connect needs to publish Pegasus, generated from the
repo so it can be re-made for every release:

| What | Where | Made by |
|---|---|---|
| Listing copy, categories, age rating, privacy labels, review notes, distribution settings | `listing.md` | hand-written, character limits checked |
| App icon (1024×1024, opaque) | `../Pegasus/Assets.xcassets/AppIcon.appiconset/AppIcon1024.png` | rendered from `icon.svg`; ASC takes it from the build, no upload |
| Screenshots — raw captures | `screenshots/raw/<set>/` | `screenshots.mjs` (headless Chromium driving the real built site) |
| Screenshots — captioned store sets | `screenshots/final/<set>/` | `compose.mjs` (headline + sub over the capture, same pixel size) |
| Privacy policy page | `https://pegasusmoonlander.com/privacy.html` | `privacy.html` at the repo root, served with the site |
| The build | TestFlight | `ios-testflight.yml` via the `vX.Y.Z` tag push (see CLAUDE.md "Versioning") |

Sets and sizes. Which iPhone slot App Store Connect marks as required
depends on the app record — newer records ask for 6.9", older ones for
6.5" — so both are generated; the other sizes scale down from whichever
is filled:

| Set | Pixels | Upload slot |
|---|---|---|
| `iphone-6.9` | 1320 × 2868 portrait | iPhone 6.9" Display |
| `iphone-6.9-landscape` | 2868 × 1320 | same slot (optional extras) |
| `iphone-6.5` | 1284 × 2778 portrait | iPhone 6.5" Display |
| `iphone-6.5-landscape` | 2778 × 1284 | same slot (optional extras) |
| `ipad-13` | 2064 × 2752 portrait | iPad 13" Display |
| `ipad-13-landscape` | 2752 × 2064 | same slot (optional extras) |

Upload the iPhone set your record asks for (`final/iphone-6.9/` or
`final/iphone-6.5/`) and `final/ipad-13/*.png` in file order
(the numbering is the story: flight first, then levels, boards, replay,
controls — the first three appear in search results). The landscape
folders are extras for the same slots; up to 10 images per slot.

## Regenerating

```bash
GIT_REV=$(git rev-parse --short HEAD) tools/build-site.sh     # the real site into site/
curl -o site/config.json https://pegasusmoonlander.com/config.json   # optional: live boards, ghost, replays
cd ios/app-store && npm ci
npm run shots        # → screenshots/raw/   (~5 min, all sets, both orientations)
npm run compose      # → screenshots/final/
```

`npm run shots` serves `site/` on a local port, opens it at each device's
CSS size and pixel ratio with touch enabled, and walks the menu: home →
level picker → a short burn on The Expanse (throttle button + stick under
two synthetic fingers) → the same on The Hollows → the Expanse all-time
board → the record run's replay with the transport bar → Settings →
Flight manual. Without `site/config.json` the board/replay shots are
skipped and the flight shots show no ghost/record — still valid, just
emptier. Knobs: `DEVICE=iphone-6.9` (one set), `NO_LANDSCAPE=1`, `SET=iphone-6.5` on
`compose` (one set + its landscape),
`CHROMIUM_PATH=…` (reuse a preinstalled Chromium instead of Playwright's
download), `RELAY=1` (route the backend through the script via curl —
for sandboxes whose egress proxy the browser does not trust).

Captions live in `CAPTIONS` at the top of `compose.mjs`; the raw folder
is the honest, uncaptioned fallback if a reviewer ever objects to
overlaid text (Apple allows captions as long as the app is shown as it
runs — these are unedited captures inside a frame).

The captures come from the web build, which is byte-for-byte the build
the app bundles (`sync-web.sh`), so they show the real app — minus the
iOS status bar and the safe-area insets, which the store does not require
in screenshots.

## Submission checklist

1. **Tag the release** (`git tag -a v1.0.0 -m v1.0.0 && git push origin v1.0.0`
   — `v1.0.0` is already on `main`; the tag push builds and uploads both
   store apps). Wait for TestFlight processing; the build number is the
   workflow run number.
2. App Store Connect → My Apps → Pegasus (Apple ID 6792584910) → **App
   Information**: fill from `listing.md` (name, subtitle, categories,
   content rights, age rating, privacy policy URL).
3. **App Privacy**: answer the questionnaire from the table in `listing.md`
   ("Data Not Linked to You": Product Interaction, Other Usage Data, Crash
   Data, User ID, Gameplay Content, Other User Content; Tracking: No),
   then Publish the labels.
4. **Business → Digital Services Act**: declare trader status (non-trader
   for a free hobby app) or the app is hidden in EU storefronts.
5. **1.0 Prepare for Submission**: screenshots (both required slots),
   promotional text, description, keywords, support + marketing URLs,
   version = the tag (`1.0.0`), copyright, select the TestFlight build,
   App Review contact + notes from `listing.md`, release = manual.
6. Optional: add the Swedish localization from `listing.md`.
7. Submit. After approval, release manually; the Safari Smart App Banner
   and the manifest's related-app entry activate by themselves.
