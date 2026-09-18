# Pegasus Android app

The Android twin of `ios/`: a thin Kotlin shell that bundles the web build
into an offline-capable app. The game runs unmodified in a full-screen
`WebView`; bundled files are served through `WebViewAssetLoader` on the
reserved `https://appassets.androidplatform.net` origin — needed for the
same reason as the iOS `pegasus://` scheme handler (`fetch()` and
localStorage need a real secure origin). Online high scores, the ghost and
analytics work over the network when the bundled `config.json` is present.
System back steps one screen in the game UI, exactly like the website
already does in Android browsers.

## Build & run locally

Prerequisites: JDK 17+, Gradle 9.5+ (or run `gradle wrapper` once and use
`./gradlew`), the Android SDK (easiest via Android Studio), and the Rust
toolchain.

```bash
./android/sync-web.sh                 # build wasm, assemble assets/webroot/
gradle -p android assembleDebug       # or open android/ in Android Studio
adb install android/app/build/outputs/apk/debug/app-debug.apk
```

The debug APK installs on any device with "install unknown apps" enabled —
no accounts, no signing setup. Re-run `sync-web.sh` + rebuild after game
changes.

## CI (GitHub Actions)

- **`android-build.yml`** — on every PR touching `android/`: builds a
  debug APK on an ubuntu runner and attaches it as an artifact, so every
  PR produces a directly installable build. It then builds the unsigned
  release AAB + APK twice and `cmp`s them, proving the Gradle half of a
  release reproducible (see "Reproducing a release build"). No secrets
  needed.
- **`android-smoke.yml`** — on every PR touching `android/`: boots the
  debug APK on an emulator, screenshots it and dumps logcat (fails on a
  `FATAL EXCEPTION`). Added after a boot crash that couldn't be diagnosed
  without a stack trace; the screenshot + logcat are always uploaded.
- **`android-release.yml`** — on **manual dispatch only** (Actions →
  Android release → Run workflow, or release both apps at once via the
  **Release apps** wrapper workflow; automatic publishing on `main`
  pushes is paused since 2026-08 — the push trigger and its deliberately
  inverted `paths-ignore` filter are kept commented out in the workflow
  for when it resumes): builds a **signed AAB + universal APK**
  (artifacts), **publishes the APK to GitHub Pages** at
  `https://pegasusmoonlander.com/app/pegasus.apk` (a public
  direct-download sideload link, refreshed every release run), and uploads
  the AAB to the **Play internal testing track** once
  `PLAY_SERVICE_ACCOUNT_JSON` is configured (the step is skipped until
  then). `versionCode` = the workflow run number; `versionName` = the
  nearest `vX.Y.Z` tag via `tools/version.sh --marketing` (the run fails
  on an untagged history — a store release is always cut from a tagged
  commit, and pushing a tag dispatches this workflow through the Release
  apps wrapper). The signed AAB and APK are attested with
  `actions/attest-build-provenance` (see "Verifying a build"). **Internal
  is where a release stops** (owner decision 2026-09, after the first Play release):
  promoting a build to closed/open testing or production is a manual
  "Promote release" in Play Console. The dispatch form's `track` input
  (`android_track` on the Release apps wrapper) can aim one run straight
  at `alpha` (closed testing) or `beta` (open testing) instead; a tag
  push never sets it.
- **`android-test-apk.yml`** — **opt-in per PR**: add the **`test-apk`
  label** to a pull request and it builds an installable APK, published at
  `https://pegasusmoonlander.com/pr-<n>/app/pegasus.apk` and
  linked from a sticky PR comment. While the label stays on, every push to
  the PR refreshes it; a manual dispatch takes a PR number instead. Most
  PRs never need a device build, which is why it isn't automatic.

  The build is the **`preview` type**: the release app under the
  applicationId `se.danielfalk.pegasus.preview`, labelled **"Pegasus PR"**
  in the launcher. That means a tester installs it **alongside** the real
  app — nothing is replaced, no uninstall, and the installed app keeps its
  settings, pilot name and cached boards. It's signed with the same upload
  key, so re-testing a PR upgrades the test app in place. Check that
  About → **Version** reads `<tag>-pr<n> (<run>)` — e.g. `1.0.0-pr12 (57)`,
  or `0.0.0-pr12 (57)` on an untagged branch — before testing; Android
  will otherwise happily serve a cached APK and the test proves nothing.
  The Source revision row shows the PR's head sha with a `-pr-<n>` marker.

  The APK lands **inside** the PR's preview directory, so
  `preview-teardown.yml` deletes it along with the rest of the preview when
  the PR closes, and the main `app/pegasus.apk` download is never touched.

### Signing secrets (one-time)

Generate an upload keystore locally (any machine with a JDK):

```bash
keytool -genkeypair -v -keystore pegasus-upload.jks -alias pegasus \
  -keyalg RSA -keysize 2048 -validity 10000
base64 -i pegasus-upload.jks | pbcopy       # macOS; on Linux: base64 -w0
```

Repository secrets (Settings → Secrets and variables → Actions):

| Secret | Value |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | the base64 of `pegasus-upload.jks` |
| `ANDROID_KEYSTORE_PASSWORD` | keystore password |
| `ANDROID_KEY_ALIAS` | `pegasus` (or whatever you chose) |
| `ANDROID_KEY_PASSWORD` | key password (often = keystore password) |

Keep the `.jks` backed up privately (password manager). With Play App
Signing (the default), Google holds the real app signing key and this is
only the *upload* key — losable and resettable via Play support, but a
hassle.

## Google Play (one-time)

1. Register a Play Console developer account ($25, one-time) at
   [play.google.com/console](https://play.google.com/console).
2. **Create app** → name "Pegasus", app/game, free. Complete the content
   declarations (privacy, ads = none, content rating questionnaire).
3. **First upload is manual** (Google requirement): run the Android
   release workflow, download the `pegasus-release` artifact, and upload
   `app-release.aab` under Testing → Internal testing → Create release.
   Accept Play App Signing when offered.
4. **Automate uploads**: Play Console → Setup → API access → link a Google
   Cloud project → create a service account with the *Release manager*
   role, download its JSON key, and paste the whole JSON into the
   `PLAY_SERVICE_ACCOUNT_JSON` repo secret. From then on every release
   workflow run uploads its build to the internal track by itself (runs
   are manual-dispatch only while automatic publishing is paused).
5. **Wider testing/production**: personal accounts created after 2023
   must run a closed test (≥ 12 testers for 14 days) before applying for
   production access (done — the 2026-08 closed test fed the alpha track
   directly via what is now the `track` input). Internal testing (up to
   100 testers by email) works immediately, and the signed APK artifact
   sideloads freely regardless. Since the first Play release every run
   lands on internal and is promoted from there in the console.

## Reproducing a release build

A release is a function of the commit, the build number, the marketing
version and the backend config it bundled (`android-build.yml` proves the
Gradle half on every PR: an unsigned rebuild must be byte-identical). To
rebuild build `N` of a tagged commit and compare it with the workflow's
`pegasus-release` artifact:

```bash
git checkout v1.0.0            # the tag (or the exact commit the run built)
PEGASUS_BACKEND_CONFIG="$(curl -fsS https://pegasusmoonlander.com/config.json)" \
  ./android/sync-web.sh        # the config.json the run bundled (same value as the repo variable)
PEGASUS_VERSION_CODE=N PEGASUS_VERSION_NAME=1.0.0 gradle -p android bundleRelease assembleRelease
```

Your outputs are unsigned; the artifact is signed with the upload key.
Signing only ADDS entries (`META-INF/*` in the AAB, the APK signing block
plus `META-INF/` in the APK), so compare the zip contents entry by entry
and ignore those:

```bash
unzip -v app-release.aab | grep -v META-INF   # name, size, CRC-32 per entry — diff the two listings
```

The wasm inside `assets/webroot/` is the same bytes the website serves for
that commit (`tools/build-wasm.sh`), and the release workflow attests the
signed AAB and APK — see "Verifying a build" below.

## Verifying a build

Every release run signs a **provenance attestation** for the AAB and the
APK (`actions/attest-build-provenance`, stored under the repo's
Attestations tab): a Sigstore-signed statement that these exact bytes were
produced by `android-release.yml` at a given commit and run. The sideload
APK is served exactly as built, so anyone can check it:

```bash
curl -fsSLO https://pegasusmoonlander.com/app/pegasus.apk
gh attestation verify pegasus.apk --repo dannyrhubarb/pegasus
```

The output names the workflow, the commit and the run number (= the
build number in parentheses on About → Version). Play re-signs what it
distributes, so
a store-installed APK is not comparable bytes; the AAB attestation covers
the artifact that was uploaded.

## Gotchas

- **`assets/webroot/` is gitignored** (build product, like the wasm); the
  `.gitkeep` holds the folder.
- The asset handler answers **real 404s** for missing optional files
  (`config.json`, `version.json`, `whats-new.json`) — returning null would
  push the request to the network where the reserved domain fails DNS and
  fetch errors instead. Don't "simplify" it to the stock
  `AssetsPathHandler`.
- `android:configChanges` keeps the activity alive across rotation — an
  activity recreate would reload the page and kill the run mid-flight.
- System bars stay visible (transparent, over the game). Don't hide the
  navigation bar with swipe-to-reveal: any upward swipe near the bottom —
  where the stick lives — reveals the bars and the next touch is eaten
  dismissing them.
- Targeting SDK 36 enables predictive back, which stops delivering
  `onBackPressed()`; the manifest opts out
  (`android:enableOnBackInvokedCallback="false"`) so system back keeps
  driving the game's screen stack. A future Android release will drop
  that opt-out — migrate to `OnBackInvokedDispatcher` then.
- Kotlin is built into AGP 9 — don't apply `org.jetbrains.kotlin.android`
  (it conflicts). The Gradle version is pinned in the four android
  workflows (`gradle-version:`), the only place it lives (no wrapper in
  the repo).
- The injected revision is the plain commit sha (the old `-android`
  suffix was dropped 2026-09 — analytics already tags these sessions as
  Android webview in the device-mix enums, and the About screen's Version
  row shows the installed app's version). The PR test APK keeps its
  `-pr-<n>` marker via `PEGASUS_REV`.
- `config.json` comes from `PEGASUS_BACKEND_CONFIG` (CI passes the
  `BACKEND_CONFIG_JSON` repo variable, the same JSON the web deploy
  writes); unset, `sync-web.sh` falls back to fetching the live site's
  copy so a local build still gets online scores.
- Launcher icons are rendered from the repo's `icon.svg` (adaptive
  foreground at 108dp densities + legacy sizes); re-render if it changes.

## App Links

`AndroidManifest.xml` carries an `autoVerify` intent filter for
`https://pegasusmoonlander.com/` (root + `index.html` only); the site
serves `.well-known/assetlinks.json` with the Play app-signing
certificate's SHA-256, so only the Play-signed release build verifies —
the `.preview` test APK and locally signed debug builds keep opening links
in the browser. Check on a device: `adb shell pm get-app-links
se.danielfalk.pegasus`.
