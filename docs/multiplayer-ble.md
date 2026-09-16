# Nearby multiplayer over BLE GATT — the spike

Status: **SPIKE** (2026-09). Feasibility work for a "race the friend sitting
next to you" mode that needs no internet, no shared Wi-Fi and no room code.
It answers one question: *can the two app shells carry the #148 wire
protocol over a custom Bluetooth Low Energy GATT link, cross-OS, at the
cadence the shadow race needs?* It is NOT a game mode — nothing here
touches the wasm, the sim, the recording format or the backend.

Read `docs/multiplayer-p2p.md` first: the 2-player shadow race (PR #148)
is the product; this spike only proposes a second TRANSPORT underneath it.

## Why Bluetooth has to be native (what the web can't do)

- **Web Bluetooth is central-only**: a page can connect to a BLE peripheral
  but can never advertise or BE one, so two phones running the website
  cannot see each other at all. It also ships only in Chromium browsers on
  Android/desktop — Safari/WebKit and Firefox refuse it, so no iOS browser
  and neither WKWebView nor Android WebView expose it.
- **No web API for nearby discovery** (no mDNS/DNS-SD browsing, no UDP
  broadcast, the Local Peer-to-Peer proposal has not shipped).
- So a nearby mode is an **app-shell feature only**, and the website can
  never have it. The web is the primary platform (every push to `main`
  deploys it; the store apps trail behind manual releases), which is the
  one real argument against investing here — the "cheap nearby wins" in
  #148's own terms (invite link via the share sheet, a QR code of it, the
  warm-invite TODO in both shells) reach every platform including the web.

## Why GATT rather than the platform frameworks

MultipeerConnectivity (iOS) and Nearby Connections (Android) each give
discovery + a reliable ordered byte session for free — but **they do not
interoperate**, and an Android ↔ iPhone pair is the common case. A custom
GATT service is the one link both OSes speak natively:

| | Android | iOS |
|---|---|---|
| Peripheral (advertise + GATT server) | `BluetoothLeAdvertiser` + `BluetoothGattServer` (most phones since 5.0; `bluetoothLeAdvertiser` is null on the few that can't) | `CBPeripheralManager` — **foreground only**, backgrounded apps advertise without the local name and only to iOS centrals |
| Central (scan + client) | `BluetoothLeScanner` + `BluetoothGatt` | `CBCentralManager` |
| Bluetooth Classic (RFCOMM) | yes | **no** (MFi only) — so cross-OS means BLE |

Budget check against the #148 protocol (~1 KB/s: input-change batches
tick-stamped at 10–20 Hz + 48–60 B keyframes at 1 Hz):

- BLE 4.2+ with data-length extension moves tens of KB/s; the default
  ATT MTU (23 → 20 B payload) already fits a batch per PDU, and both
  centrals negotiate up (Android asks for 517; iOS reports what it got).
- Connection intervals of 15–30 ms sit under the batch cadence, so a
  batch normally rides the next interval: **one-way latency ≈ one
  interval + the bridge hop**, well inside the "where you SEE the
  opponent" tolerance (the opponent is a ghost; latency never touches
  your own physics).

## The link design (implemented by the spike)

**Roles are fixed per side**, mapping onto #148's host/guest:

- **Host = GATT peripheral**: advertises the service, accepts one central,
  sends with **notifications** on TX, receives **writes-without-response**
  on RX.
- **Guest = GATT central**: scans for the service UUID, connects,
  subscribes to TX, writes RX.

Service (custom 128-bit UUIDs, shared by both bridges and the page):

| | UUID | Props |
|---|---|---|
| Service | `7E6A5000-0148-4B1E-8F3A-000000000001` | primary |
| TX (host → guest) | `…0002` | notify (+ CCCD `0x2902`) |
| RX (guest → host) | `…0003` | write without response |

**Native is a dumb PDU pipe, the page owns the codec** — one implementation
of framing for both platforms, and the shells stay small:

- A `send` command = exactly ONE GATT notification / write of ≤ (mtu − 3)
  bytes, delivered in order (Android: strictly one PDU in flight, the next
  goes out on `onNotificationSent` / `onCharacteristicWrite`; iOS:
  `updateValue` false → wait for `peripheralManagerIsReady`,
  `canSendWriteWithoutResponse` false → wait for `peripheralIsReady`).
- The page (`pegBle` in `index.html`) frames every message as
  `[u16 BE len][bytes]`, chunks the framed stream to the payload size, and
  reassembles on the far side. Any chunking is valid — the receiver just
  concatenates PDUs and parses frames.
- Discovery name: the callsign rides the **scan response as service data**
  on Android (≤ 12 B — the adapter's device name is the phone's Bluetooth
  name, and changing it is global) and as the **local name** on iOS
  (CoreBluetooth can't advertise service data). Each central reads
  whichever is present; the reliable callsign exchange is the HELLO frame
  after connect.

### Bridge contract (`window.pegBle` ↔ the shells)

Commands (page → native, one JSON string each — Android
`PegasusBle.cmd(json)`, iOS `webkit.messageHandlers.pegasusBle.postMessage`):

| cmd | args | effect |
|---|---|---|
| `host` | `name` | become the peripheral: build the service, advertise |
| `scan` | | become the central: scan for the service |
| `connect` | `id` (address / peripheral identifier from a `peer` event) | stop scanning, connect, subscribe |
| `send` | `b64` | one PDU (≤ mtu − 3 bytes) |
| `stop` | | tear everything down |

Events (native → page, `pegBle._on({ev, …})` via `evaluateJavascript`):

| ev | fields |
|---|---|
| `state` | `state` ∈ idle · advertising · scanning · connecting · connected · disconnected, `role`, `mtu` (on connected), `reason` |
| `mtu` | `mtu` (a later renegotiation) |
| `peer` | `id`, `name`, `rssi` |
| `data` | `b64` (one received PDU) |
| `error` / `log` | `msg` |

Feature detection: Android exposes the `PegasusBle` interface; iOS injects
`window.__pegBleIos = true` at document start (postMessage has no
synchronous return). The plain website has neither → `pegBle.available()`
is false and nothing runs. **Neither shell touches the radio before the
first host/scan command** — creating a `CBCentralManager` or asking for
Android's Bluetooth permissions triggers the system prompt, and the game
must never prompt at launch.

Permissions: Android 12+ `BLUETOOTH_SCAN` (flagged `neverForLocation`),
`BLUETOOTH_ADVERTISE`, `BLUETOOTH_CONNECT`, requested lazily and the
interrupted command re-run on grant; ≤ 11 falls back to
`ACCESS_FINE_LOCATION` (declared with `maxSdkVersion="30"` — **a launch
decision**: Play flags location permissions, and dropping pre-12 scanning
would avoid declaring it at all). iOS: `NSBluetoothAlwaysUsageDescription`
in Info.plist; the prompt appears on the first manager creation.

## The spike screen (Settings → "Nearby (BLE spike)")

Shown only in the app shells while the **Debug HUD** toggle is on
(`#btn-ble`, the same switch as the consent-reset button). `scr-ble`:

1. **Host (advertise)** on phone A, **Find nearby hosts** on phone B → A's
   callsign appears in B's list with RSSI → tap → both show `connected ·
   <role> · mtu N (P B/pdu) · peer <callsign>` (HELLO frames both ways).
2. **Ping ×20** — 20 round trips 100 ms apart, logs `rtt min / med / max`.
3. **Stream 10 s @ 15 Hz** — 150 × 16-byte frames (the shape of a
   tick-stamped input batch); the receiver reports back `got N/150 · gap
   mean / max` and the sender logs the report.
4. **Stop** on either side → the other shows `disconnected`.

Every line also goes to `pegLog`, so a bug-report zip carries the numbers.

### What a device session should record (the numbers the decision needs)

Run each pairing both ways (who hosts matters: iOS-as-peripheral is the
constrained role):

| pair | mtu | rtt med / max | stream got / gap max | notes |
|---|---|---|---|---|
| Android → Android | | | | |
| Android host → iPhone guest | | | | |
| iPhone host → Android guest | | | | |
| iPhone → iPhone | | | | |

Pass = every stream 150/150 with gap max < ~150 ms and rtt med < ~100 ms
on all four; then the #148 feed (10–20 Hz batches + 1 Hz keyframes) has
headroom. Also note: does the link survive the host phone's screen
locking? (iOS foreground-only peripheral is the expected failure.)

## Headless check (`tests/ble-spike/`)

`node run.mjs` opens two pages against the real `index.html`, injects a
FAKE `PegasusBle` bridge into each, and plays the radio in Node — the
fake enforces the ATT-default 20-byte payload and fails on any oversized
PDU. It proves the page half end to end: host → find → connect → hello →
ping burst → 10 s stream → report, plus a 1000-byte message crossing in
51 PDUs byte-exact, and stop → peer disconnect. The real bridges can only
be exercised on devices (there is no BLE in CI); the fake keeps the
shared codec honest between those sessions. Not wired into CI for now
(a spike); run it by hand after touching `pegBle` or the screen.

## Verification status (be honest about this)

- **Page half**: headless test green (see above).
- **Android bridge** (`BleBridge.kt`): written against API 26–36 with the
  33+ / pre-33 GATT call variants, **not compiled in this session** (no
  Android SDK here) — `android-build.yml` compiles it on the PR, and the
  `test-apk` label builds an installable preview APK.
- **iOS bridge** (`BleBridge.swift`): written, registered in the pbxproj,
  **not compiled in this session** (no Xcode here) — `ios-build.yml` runs
  the unsigned build on the PR.
- **No device pairing yet** — the table above is empty until someone runs
  it on two phones.

## If the spike passes: how it plugs into #148

1. In `pegMP`, split the DataChannel out behind a tiny transport interface
   (`send(bytes)`, `onmessage`, `onclose`, `close()`); the WebRTC channel
   is implementation one, `pegBle` is implementation two — both are
   reliable, ordered byte-message channels, which is all the JSON control
   messages and the binary batch stream ever needed.
2. Discovery replaces signaling: no room code, no `wsUrl`, no TURN. The
   host advertises, the guest picks from the list — the `scr-mp-host` /
   `scr-mp-join` screens gain a "Nearby" path; the feature-gate becomes
   `wsUrl OR pegBle.available()`.
3. Everything from `hello` on is unchanged: level text + concrete seed,
   `ready`/`start`, batches + keyframes into `RemoteFeed`, the respawn
   marker, `run_end`. Treat bridge bytes as attacker-controlled exactly
   like DataChannel bytes (the `ingest_batch` caps already do).
4. Keep the DataChannel path as the default; nearby is the store-app extra.

## Out of scope for the spike

Pairing more than two phones, background operation, a Bluetooth-Classic
Android-only fast path, reconnection after a drop (a dropped link degrades
to a solo run in #148 terms), and any wasm or sim-core change.
