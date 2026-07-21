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

Prerequisites: JDK 17+, Gradle 8.11+ (or run `gradle wrapper` once and use
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
  PR produces a directly installable build. No secrets needed.
- **`android-release.yml`** — on manual dispatch and `main` pushes
  touching `android/`: builds a **signed AAB + universal APK**
  (artifacts), and uploads the AAB to the **Play internal testing track**
  once `PLAY_SERVICE_ACCOUNT_JSON` is configured (the step is skipped
  until then). `versionCode` = the workflow run number.

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
   `PLAY_SERVICE_ACCOUNT_JSON` repo secret. From then on the release
   workflow uploads every build to the internal track by itself.
5. **Wider testing/production**: personal accounts created after 2023
   must run a closed test (≥ 12 testers for 14 days) before applying for
   production access. Internal testing (up to 100 testers by email) works
   immediately, and the signed APK artifact sideloads freely regardless.

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
- The injected revision is suffixed **`-android`**; analytics tags these
  sessions as Android webview device-mix.
- Launcher icons are rendered from the repo's `icon.svg` (adaptive
  foreground at 108dp densities + legacy sizes); re-render if it changes.
