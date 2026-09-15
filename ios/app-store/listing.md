# Pegasus — App Store Connect listing

Everything App Store Connect asks for at **App Store → 1.0 Prepare for
Submission**, ready to paste. Character limits are Apple's (2026). The
numbered screenshot sets are in `screenshots/` (see `README.md`).

Fixed facts (from the project, not choices):

| Field | Value |
|---|---|
| Apple ID (ASC app record) | 6792584910 |
| Bundle ID | `se.danielfalk.pegasus` |
| SKU | `pegasus-ios` (any unique string; suggest this) |
| Primary language | English (U.S.) |
| Marketing version | the newest `vX.Y.Z` tag on `main` (`v1.0.0` today) — set by `tools/version.sh --marketing` on the TestFlight archive; the store's "Version" field must match it |
| Build | the TestFlight build uploaded by `ios-testflight.yml` (build number = the run number) |
| Minimum iOS | 15.0 (`IPHONEOS_DEPLOYMENT_TARGET`) |
| Devices | iPhone + iPad (`TARGETED_DEVICE_FAMILY = 1,2`), portrait + landscape |
| Price | Free, all territories |
| Export compliance | Handled: `ITSAppUsesNonExemptEncryption = false` in Info.plist (HTTPS only) |
| App icon | Comes from the build (`AppIcon1024.png` in `Assets.xcassets`, 1024×1024, opaque RGB — no separate upload since Xcode 14) |

---

## App Information

**Name** (30 max)

    Pegasus — Moon Lander

**Subtitle** (30 max)

    Thrust, steer, land. Repeat.

**Primary category**: Games
**Game subcategories**: Arcade, Simulation
**Secondary category**: (none)

**Content rights**: "No, it does not contain, show, or access third-party
content." (Player callsigns on the leaderboards are user-entered text, not
licensed third-party content.)

**Age rating** — Apple's questionnaire, answer every item "None" except:

| Question | Answer | Why |
|---|---|---|
| Cartoon or fantasy violence | Infrequent / Mild | The ship explodes into debris on a crash; nothing else |
| Unrestricted web access | No | Bundled pages only; http(s) links open in Safari, not in-app |
| User-generated content / user interaction | Users can post: yes, limited (names on a public leaderboard); no messaging, no profiles, no direct contact | Pilot callsigns and replays on the global boards. Apple's UGC review asks for filtering + a report path: names are capped at 24 characters and the privacy page names the GitHub issue tracker for takedowns. Expect **4+**; if the reviewer rates the UGC answer higher (9+/12+) accept it — it changes nothing else. |
| Gambling, contests, loot boxes, medical, mature themes, etc. | No | — |

**Copyright**

    © 2026 Daniel Falk

**Support URL**

    https://github.com/dannyrhubarb/pegasus/issues

**Marketing URL** (optional)

    https://pegasusmoonlander.com

**Privacy Policy URL**

    https://pegasusmoonlander.com/privacy.html

**Routing app coverage file**: none. **Sign-in required**: No.

---

## 1.0 — Version Information (English U.S.)

**Promotional text** (170 max — editable without a new build)

    Fly a fragile lander through twisting caves, land on fuel pads, and race the world record as a ghost that flies right beside you.

**Description** (4000 max)

    Pegasus is a moon-lander game with real physics. Pilot a fragile ship through cave systems that twist, narrow and open into vast chambers. Thread the rock, dodge boulders, watch your fuel, and set the ship down gently on the landing pads before the tank runs dry.

    ONE STICK, REAL PHYSICS
    Hold to burn, point to steer. The ship rotates the short way to where you point it and the engine only fires while you hold. Two-handed split controls (throttle left, stick right) and a left-handed layout are one tap away in Settings. Game controllers and keyboards work too.

    TEN LEVELS, THREE WAYS TO SCORE
    • Distance runs — fly as far as you dare in either direction.
    • Sprints — sixty seconds on the clock; every metre counts.
    • Dashes and time trials — race 1,000 m to the finish pad, or visit every pad in The Hollows, a hand-drawn cave of five chambers.
    The Flux reshuffles its endless cave on every attempt, so no two runs are alike.

    LAND LIKE YOU MEAN IT
    Touch down slow, level, with both feet on the deck and hold it — a settle ring shows exactly when the landing counts. Come in too hot and the hull takes damage; too hot again and you are debris. Pads refuel and repair while you sit on them.

    RACE THE WORLD RECORD
    Global high-score boards for every level — today, this week, all time. The record holder's run flies alongside you as a translucent ghost, and every board entry has a replay you can watch, pause, scrub and slow down frame by frame. Submit your own runs under any callsign you like; every score is verified by re-simulating the replay, so the boards are honest.

    PLAYS OFFLINE
    No account, no ads, no in-app purchases. The whole game ships inside the app; the boards, ghost and replays need a connection, everything else works in a tunnel.

    Fly safe. Or don't — the replay is worth watching either way.

**Keywords** (100 max, comma-separated, no spaces after commas)

    moon lander,lunar,cave,physics,arcade,retro,replay,ghost,thrust,gravity,time trial,spaceship

**What's New in This Version** (4000 max)

    First App Store release: ten levels, global high scores with verified replays, and the racing ghost.

**Screenshots**: upload `screenshots/final/iphone-6.5/*.png` to the
**iPhone 6.5" Display** slot (1284×2778 — the size this app record asks
for; `iphone-6.9/` at 1320×2868 is there too if the slot ever changes)
and `screenshots/final/ipad-13/*.png` to the **iPad 13" Display** slot,
in the numbered order. Every other size scales down from those unless you
untick "Use … screenshots for …". Up to 10 per set; the first three
show in search results.

**App Preview** (video): optional, none provided. If wanted later: 15–30 s,
same pixel sizes as the screenshots, H.264, captured on device with the
built-in screen recorder.

---

## 1.0 — Version Information (Swedish, optional localization)

Add the "Swedish" localization and paste these; leave everything else
falling back to English.

**Name**: `Pegasus — Moon Lander`   **Subtitle**: `Gasa, styr, landa. Igen.`

**Promotional text**

    Flyg en ömtålig månlandare genom slingrande grottor, landa på bränsleplattor och tävla mot världsrekordet som flyger bredvid dig som ett spöke.

**Description**

    Pegasus är ett månlandarspel med riktig fysik. Styr ett ömtåligt skepp genom grottsystem som vrider sig, smalnar av och öppnar upp i stora kammare. Tråckla dig mellan klipporna, undvik stenblocken, håll koll på bränslet och sätt ner skeppet mjukt på landningsplattorna innan tanken tar slut.

    EN SPAK, RIKTIG FYSIK
    Håll för att bränna, peka för att styra. Skeppet vrider sig kortaste vägen dit du pekar och motorn brinner bara medan du håller. Delade kontroller (gas till vänster, spak till höger) och vänsterhänt läge finns i inställningarna. Handkontroller och tangentbord fungerar också.

    TIO BANOR, TRE SÄTT ATT TÄVLA
    • Distans — flyg så långt du vågar åt valfritt håll.
    • Sprint — sextio sekunder på klockan, varje meter räknas.
    • Dash och tidslopp — kör 1 000 m till målplattan, eller besök alla plattor i The Hollows, en handritad grotta med fem kammare.
    The Flux blandar om sin oändliga grotta vid varje försök, så inga två flygningar är lika.

    TÄVLA MOT VÄRLDSREKORDET
    Globala topplistor för varje bana — idag, den här veckan, genom tiderna. Rekordhållarens flygning följer med dig som ett genomskinligt spöke, och varje rad på listan har en repris du kan pausa, spola och köra i slow motion. Skicka in dina egna flygningar under vilket anropsnamn du vill; varje resultat verifieras genom att reprisen simuleras om, så listorna är ärliga.

    FUNGERAR OFFLINE
    Inget konto, ingen reklam, inga köp i appen. Hela spelet ligger i appen; topplistor, spöke och repriser behöver uppkoppling, allt annat funkar i en tunnel.

**Keywords**

    månlandare,grotta,fysik,arkad,retro,repris,spöke,rymdskepp,tidslopp,lunar lander

---

## App Privacy (the "nutrition label")

Source of truth: `privacy.html` and the analytics section of `CLAUDE.md`.
Fill the questionnaire like this. **Tracking: No** (nothing is shared for
advertising or with data brokers; no IDFA).

| Data type (Apple category) | Collected? | Linked to the user? | Used for tracking? | Purpose | Notes |
|---|---|---|---|---|---|
| Usage Data → Product Interaction | Yes | No | No | Analytics | Runs, distances, menu taps, screens viewed — sent to the developer's own server, anonymous per-session id |
| Usage Data → Other Usage Data | Yes | No | No | Analytics | Coarse device class (iOS / phone), the referring site's origin, utm tags |
| Diagnostics → Crash Data | Yes | No | No | Analytics, App Functionality | Script/wasm error messages |
| Identifiers → User ID | Yes (only after opt-in) | No | No | Analytics | The optional random "returning-player id" — off until the player accepts the in-game prompt; deleted when switched off |
| User Content → Gameplay Content | Yes (only when the player submits a score) | No | No | App Functionality | Score + replay recording of the run, shown publicly on the boards |
| User Content → Other User Content | Yes (only when the player submits a score) | No | No | App Functionality | The self-chosen pilot callsign shown next to the score |
| Everything else (contacts, location, purchases, health, photos, browsing history, precise/coarse location, device ID, email, name, phone, …) | No | — | — | — | Not collected |

"Linked to the user" is **No** across the board: there are no accounts and
nothing ties the data to an identity; the callsign is whatever the player
types. If Apple's reviewer disagrees on User ID, "Linked" for that one row
is the only thing to flip.

---

## App Review Information

**Sign-in required**: No.

**Contact**: first name, last name, phone, email of the account holder
(Daniel Falk; use the developer-account email and a phone Apple can reach
— not stored in this repo).

**Notes** (paste)

    Pegasus is a physics moon-lander game. No sign-in, no purchases, no ads.

    How to play: tap FLY, pick any level, then on the flight screen hold the LEFT half of the screen to fire the engine and drag on the RIGHT half to point the nose. Land slowly on the grey pads with both feet down and hold still for half a second — a green ring fills to confirm. Crash or run out of fuel and a game-over screen offers Fly again / Watch replay.

    Online features (HIGH SCORES, the racing ghost, ▶ watch replays) use the developer's own backend at api.pegasusmoonlander.com; the game plays fully offline without them. Submitting a score is optional and asks for a callsign only. The optional "returning-player id" prompt appears after a few plays; declining changes nothing.

    The app bundles the same web build served at https://pegasusmoonlander.com and runs it in a WKWebView; that is by design (offline play, identical physics to the website so replays verify on the server).

**Attachment**: none needed.

---

## Distribution settings

- **Pricing**: Free. **Availability**: all territories. Pre-orders: no.
- **App Store Version Release**: "Manually release this version" for 1.0
  (the release of the website's `app-policy.json` `minVersion` wall, if
  any, should follow the store release, not precede it).
- **Phased release**: off (small audience).
- **EU Digital Services Act — trader status**: this is a free,
  non-commercial hobby app with no monetization → declare **non-trader**
  (App Store Connect → Business → Digital Services Act). Apple hides the
  app in EU storefronts until the declaration is made. If it ever gains
  purchases or ads, this must become "trader" with a published address and
  contact.
- **Agreements**: only the free *Paid Apps* agreement is NOT needed; make
  sure the Apple Developer Program License Agreement is accepted (Business
  → Agreements) or the build cannot be submitted.
- **Game Center**: not used (leave off). **App Clips / Widgets**: none.
- **Associated Domains**: Universal Links are already in the entitlements
  (`applinks:pegasusmoonlander.com`) — nothing to enter in ASC.

## After approval

1. Set `app-policy.json` `storeUrls.ios` to the live listing URL if it
   differs from the pre-filled Apple ID link (it should not).
2. The Safari Smart App Banner (`apple-itunes-app` meta tag) and the
   `related_applications` manifest entry light up by themselves once the
   app is purchasable — no deploy needed.
3. Tag the next cycle when its first beta uploads (see `CLAUDE.md`
   "Versioning").
