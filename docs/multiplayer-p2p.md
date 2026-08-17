# P2P multiplayer — implementation brief

This is an instruction file for an implementation session that has BOTH
repositories attached: **dannyrhubarb/pegasus** (the game) and
**dannyrhubarb/pegasus-backend** (the AWS serverless backend). It captures
the design decisions already made with the owner (2026-08) so the
implementing session does not re-litigate them. Read both repos' CLAUDE.md
files before writing any code — pegasus's determinism rules and menu/UI
conventions, and the backend's deploy + sim-core repin rules, all apply.

## Locked decisions

1. **Game mode: 2-player shadow race.** Both players fly the SAME level
   (same world, same concrete seed) side by side, each in their own physics
   world. **No ship–ship collision** — the opponent is rendered like the
   existing racing ghost (translucent silhouette + callsign + minimap dot),
   driven live by the opponent's input stream. Latency only affects where
   you *see* the opponent, never your own physics. Shared-world collision /
   lockstep / rollback is explicitly OUT OF SCOPE.
2. **Transport: WebRTC `RTCDataChannel`, true P2P.** Game traffic goes
   browser-to-browser. ICE fallback order: direct (STUN-assisted) →
   Cloudflare TURN relay.
3. **STUN/TURN: Cloudflare.** `stun.cloudflare.com` is free/unlimited;
   TURN uses Cloudflare Realtime TURN ($0.05/GB after a 1,000 GB/month
   free tier — at this game's ~1 KB/s per player, effectively $0 forever).
   No self-hosted TURN, no AWS KVS.
4. **Signaling: serverless on pegasus-backend.** API Gateway WebSocket API
   + Lambda + a DynamoDB room table, joining the existing stack. Room-code
   UX: host gets a short code, friend types it in. The WebSocket stays
   connected for the whole match (needed to relay ICE restarts after a
   network change), but carries no game traffic.
5. **TURN credentials are minted server-side, per match.** The signaling
   Lambda calls Cloudflare's credential API and returns a ready-to-use
   `iceServers` array to both clients. The Cloudflare secrets never reach
   the browser.
6. **Scoring is unchanged.** Each player's run is still their own local
   recording; crash/fuel-out/completion still goes through the normal
   submit-score flow and backend verification. Multiplayer adds a
   compare-scores end screen, nothing more. Draft/custom levels MAY be
   raced (the host transmits the level text wholesale) but keep the
   existing guards: drafts are never submitted online and have no boards.

## Why this is cheap in this codebase

- The sim is a deterministic, fixed-timestep (`PHYSICS_DT = 1/120`) pure
  function of a quantized input stream. `InputState`
  (`sim-core/src/replay.rs`) is 5 bytes, recorded on change; keyframes are
  48–60 bytes at 1 Hz. That IS the wire protocol — well under 1 KB/s.
- The racing ghost already does remote-player rendering: `ResimPlayer` in
  `src/main.rs` steps a second `Sim` in lockstep and draws a translucent
  hull with a floated callsign. The multiplayer opponent is "a ghost whose
  recording is still being written, arriving over a DataChannel." Reuse
  that machinery; do not invent a parallel one.
- Cross-device float divergence (different libm implementations) is a
  solved problem here: the 1 Hz keyframes + the existing drift-check /
  0.5 m snap (`SNAP_DRIFT_M`) absorb it, exactly as they do for watched
  replays from other devices.

## Constraint that shapes the design: avoid sim-core changes

pegasus-backend consumes `sim-core` as a git dependency **pinned to a
`main` rev**; any change to the crate (and especially to the recording
format) forces a backend repin + redeploy before submissions verify again
(see both CLAUDE.md files). The remote-player feed can and should be built
frontend-side (in `src/main.rs`) from sim-core's existing public API —
e.g. accumulate received events into a growing `Recording` and drive a
`ResimPlayer` from it incrementally. **Do not bump
`REPLAY_FORMAT_VERSION` and do not add sim-core types for multiplayer
unless truly unavoidable**; if it becomes unavoidable, flag the repin
consequence in the PR description.

## Match flow

1. Host opens Multiplayer → Host game. Backend creates a room, returns a
   short code (4–5 chars from an unambiguous alphabet — no 0/O/1/I) +
   TURN-inclusive `iceServers`. Host picks the level (normal picker).
2. Guest opens Multiplayer → Join game, types the code. Backend pairs the
   connections, relays SDP offer/answer + ICE candidates both ways
   (opaque `signal` messages — the server never parses them).
3. DataChannel opens (ordered/reliable is fine at this bitrate). Peers
   exchange `hello` (callsign, build id/revision — warn on mismatch, the
   determinism guarantee is per-binary and cross-build races rely on the
   keyframe snap).
4. Host sends the level: the raw `.level` text + the CONCRETE seed (on
   `seed = random` levels the host rolls once and transmits — the plumbing
   for concrete seeds already exists via `LevelParams`). Both push it
   through the normal `pushLevel`/`load_level` path.
5. Both signal `ready` → synchronized 3-2-1 countdown (host picks the
   start moment; a fixed lead time like "start at T+3 s" measured over the
   DataChannel is plenty — sub-100 ms start skew is invisible in a race).
   At start, both runs arm (this REPLACES the armed-but-idle first-input
   gate for multiplayer runs; keep the recording semantics identical —
   keyframe 0 = spawn state, tick 1 = first commanded tick).
6. During flight, each peer streams: input change-events batched on a
   10–20 Hz timer (each batch tick-stamped) + its 1 Hz keyframes. The
   receiving side feeds them to the remote player; keyframes correct
   drift via the existing snap. Render the opponent ghost-style but
   visually distinct from the record ghost (different tint), callsign
   under it, own minimap dot. Both ghosts may be visible at once on
   levels that have a record ghost — that's fine.
7. Run ends (crash / fuel-out / completion / time-limit) → send
   `run_end` {cause, score}. When both have ended: a results screen
   (win/lose + both scores) with Rematch (same room, host may re-roll a
   random seed) and Exit. The normal submit-score dialog still runs for
   each player's own run afterwards, unchanged.
8. Disconnect mid-race (DataChannel closes, ICE fails beyond restart):
   banner + the race degrades to a normal solo run. Never block the
   player's own flight on network state.

## Frontend work (pegasus repo)

- **JS** (`index.html`): a self-contained multiplayer module in the main
  script, styled after the analytics module — every entry point try/caught,
  degrades to feature-off. No CDNs, no external JS. WebRTC:
  `new RTCPeerConnection({iceServers})` from the signaling response,
  `createDataChannel`, ICE-restart on `iceConnectionState` failure while
  the signaling socket is up. Feature-gate on config.json having the new
  signaling URL (absent → the Multiplayer button is hidden; local dev and
  forks stay clean, same pattern as the online-scores layer).
- **config.json**: add a `wsUrl` (WebSocket signaling URL) field. It comes
  from the backend stack output through the `BACKEND_CONFIG_JSON` repo
  variable — extend `build-site`'s validation to accept (not require) it.
  NOTE FOR THE OWNER: after the backend deploy, the repo variable must be
  re-pasted from the new `FrontendConfigJson` output (one manual step —
  the sync scripts for the iOS/Android shells fetch config.json from the
  live deployment, so the apps pick it up automatically).
- **Menu**: a `scr-multiplayer` screen (Host / Join + code entry / lobby
  status) reached from scr-home. Follow the conventions in CLAUDE.md's
  "Game menu" section to the letter: one `.screen`, `.mbtn.back` (hardware
  back then works for free), `histPath` entry for the new screen,
  `stopPropagation` on touchstart, neon styling with the existing palette
  vars, no webfonts. The results screen can be a variant of scr-gameover
  or its own screen — implementer's choice, same rules.
- **wasm bridge** (`src/main.rs`): exports following the existing byte
  buffer pattern (`blob_in_ptr` etc.): push remote input batches / remote
  keyframes; a `remote_player` fed like `ghost_player` but from the live
  stream (render gated like `ghost_pose` — hidden pre-start and when the
  remote run is over); mirrors out for the opponent's distance/state if
  the HUD wants a delta readout. Outgoing side: the recorder already
  produces the events — mirror them (and each pushed keyframe) into a
  JS-drainable buffer on the frame loop, same style as the run-analytics
  channel.
- **Determinism rules apply unchanged**: everything through `Sim::tick`,
  no frame-level physics, remote sim advanced only by ticked inputs,
  BTreeMap windows untouched. The remote player is presentation — it must
  not influence the local sim in any way.
- **App shells**: WKWebView and Android WebView both support DataChannels;
  no shell changes expected. Check `tools/check-bundle-sync.py` if any new
  file ships with the site.
- `Whats-new:` trailers on user-facing commits (see CLAUDE.md).

## Backend work (pegasus-backend repo)

Read that repo's CLAUDE.md first for stack layout and deploy flow.

- **WebSocket API**: API Gateway v2 WebSocket API (`$connect`,
  `$disconnect`, `$default` → one Lambda). Messages (JSON):
  - `create_room` → generates the code, stores
    `{code, hostConnectionId, ttl}` in a DynamoDB rooms table (TTL ~2 h,
    on-demand billing), replies `room_created` {code, iceServers}.
  - `join_room` {code} → validates, stores guest connection id, replies
    `room_joined` {iceServers}, notifies host `peer_joined`.
  - `signal` {payload} → relayed verbatim to the other connection via
    `ApiGatewayManagementApi.postToConnection`. Opaque to the server.
  - `leave` / `$disconnect` → notify the peer, clean up.
  Rate-limit room creation modestly (per-connection) to keep the code
  space unguessable in practice; codes expire with the room TTL.
- **TURN credentials**: on room create/join, the Lambda POSTs
  `https://rtc.live.cloudflare.com/v1/turn/keys/$TURN_KEY_ID/credentials/generate-ice-servers`
  with `Authorization: Bearer $TURN_KEY_API_TOKEN` and body
  `{"ttl": 21600}` (6 h — comfortably longer than any match). The response
  is a ready `iceServers` array (Cloudflare STUN + turn/turns on UDP 3478
  & 53, TCP 3478 & 80, TLS 5349 & 443 — the 443/TLS entry is what gets
  through strict firewalls; pass it to clients UNMODIFIED). Read
  `TURN_KEY_ID` / `TURN_KEY_API_TOKEN` from SSM SecureString parameters
  (created by the bootstrap workflow below) at cold start.
- **Stack output**: extend `FrontendConfigJson` with the WebSocket URL so
  the owner's one paste updates the frontend variable.
- **No changes** to score verification, events, or the sim-core pin (see
  the constraint above).

## Cloudflare setup

### Manual (owner, one-time, ~10 minutes)

1. Sign up for a free Cloudflare account (no domain, no zone needed).
2. Note the **Account ID** (dashboard sidebar / URL).
3. Create an **API token**: dashboard → My Profile → API Tokens → Create
   Custom Token → permission **Account · Calls · Edit** (Calls covers
   Realtime TURN keys). This token is only used by the bootstrap workflow.
4. In the **pegasus-backend** repo, add two GitHub Actions secrets:
   `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN`.
5. If the Realtime/TURN dashboard page asks to enable the product or add
   a payment method for overage billing, accept — usage stays inside the
   1,000 GB/month free tier at this game's traffic.

### Automated (GitHub Actions, one-time bootstrap workflow)

Add a `workflow_dispatch` workflow to pegasus-backend (e.g.
`turn-bootstrap.yml`) that:

1. **Idempotency check**: if the SSM parameters
   `/pegasus/turn/key-id` and `/pegasus/turn/api-token` already exist,
   exit successfully without touching Cloudflare. (The Cloudflare list
   endpoint never returns the secret token again, so never re-create
   blindly — a lost token means minting a NEW key and updating SSM.)
2. Creates the TURN key:
   `POST https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/calls/turn_keys`
   with `Authorization: Bearer $CLOUDFLARE_API_TOKEN`, body
   `{"name": "pegasus-multiplayer"}`. The response's `result.uid` is the
   TURN key id and `result.key` is the TURN key API token — **`key` is
   returned ONLY at creation time**.
3. Writes both to SSM as SecureString parameters using the AWS deploy
   credentials the backend workflows already have. Never echo the token
   to the job log (mask with `::add-mask::` before any output).
4. The signaling Lambda reads the two parameters at runtime; regular
   deploys need no Cloudflare access at all. Rotation = delete the SSM
   params + delete the key in Cloudflare (DELETE
   `/accounts/{account_id}/calls/turn_keys/{key_id}`) + re-run the
   workflow.

This split keeps the powerful account-level Cloudflare token in GitHub
secrets, used once, while the runtime only ever holds the narrow TURN-key
token whose only capability is minting short-lived client credentials.

**Missing-bootstrap degradation (required)**: the signaling Lambda must
treat absent SSM parameters as "TURN not configured" — log a warning and
return STUN-only `iceServers` (`stun.cloudflare.com`) instead of failing.
Most peer pairs connect without a relay, so a forgotten bootstrap weakens
the fallback rather than breaking multiplayer.

**Alternative considered and rejected** (owner decision, 2026-08): a
Lambda-backed CloudFormation custom resource in the backend stack that
creates/revokes the TURN key. It would solve deploy ordering and teardown
declaratively, but couples every stack operation (including rollbacks and
deletes) to the Cloudflare API, requires the account-level token to stay
in the deploy pipeline permanently, and custom resources wedge the stack
when a handler path fails to respond. The bootstrap workflow keeps
deploys pure-AWS; its one-time manual nature is acceptable. Do not
re-open this. (CDKTF was also considered — the Cloudflare provider's
`cloudflare_calls_turn_app` resource covers it — and rejected as a second
IaC toolchain whose state file would hold the secret.)

## Cost expectations (verified 2026-08)

- TURN: $0 (1,000 GB/month free tier; a fully-relayed hour of racing is
  ~7 MB, and only ~10–20% of peer pairs relay at all).
- Signaling: ~30–60 WebSocket messages per match setup + keepalive;
  API GW WebSocket is $1.00/M messages + $0.25/M connection-minutes +
  Lambda $0.20/M — rounds to zero at hobby scale.
- DynamoDB rooms table: on-demand, pennies.

## Testing

- **Unit (Rust)**: the remote-player feed — scripted input stream fed
  incrementally reproduces the same trajectory as feeding it as a
  finished recording; keyframe snap on an artificially diverged stream;
  no regression to the existing resim/ghost tests
  (`cargo test --workspace`).
- **E2E (Playwright, scratch-only like the existing suites)**: two pages
  in one Chromium instance + a stub signaling server; real
  RTCPeerConnection loopback works headless (host candidates — no TURN
  needed in CI). Drive: host → code → join → countdown → both fly
  scripted inputs → opponent ghost visible on both → run_end both ways →
  results screen. Reuse the `pegasus_analytics_debug` pattern for any
  automation gates (`navigator.webdriver` traffic is dropped by
  analytics — multiplayer must NOT copy that gate).
- **Manual**: phone (mobile data, NOT the same Wi-Fi — forces the
  STUN/TURN path) vs desktop; check `chrome://webrtc-internals` shows
  which candidate pair won; kill Wi-Fi mid-race to exercise the
  ICE-restart path.

## Out of scope (do not build)

Ship–ship collision / shared physics world, >2 players, matchmaking
beyond room codes, spectating, voice/chat, TURN self-hosting, relaying
game traffic through AWS.

## Left to the implementer

Exact DataChannel framing (JSON control + binary input frames is a
reasonable split), countdown UX, results-screen layout, rematch details,
reconnection niceties. Keep PRs curated per the git workflow rules; the
frontend PR gets a preview deployment for phone testing.
