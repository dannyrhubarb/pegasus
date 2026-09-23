# Pegasus

Pilot a fragile lander through a cave that twists, narrows and fills with
boulders. Thread the rock, stick the landing on refuel pads before the tank
runs dry, and chase the world record — with a translucent ghost of the
record run flying beside you.

Real 2D physics via [Rapier](https://rapier.rs), rendered with
[macroquad](https://macroquad.rs), compiled to WebAssembly. It runs in any
modern browser and ships as thin native shells for iOS and Android.

- **Play**: [pegasusmoonlander.com](https://pegasusmoonlander.com) (works as
  an installable PWA)
- **App Store**: [apps.apple.com/app/id6792584910](https://apps.apple.com/app/id6792584910)
- **Google Play**: [se.danielfalk.pegasus](https://play.google.com/store/apps/details?id=se.danielfalk.pegasus)

## Controls

| Input | Action |
|-------|--------|
| `↓` / hold left mouse button | Main engine (full throttle while held) |
| `←` / `→` | Rotate |
| `R` / ⟳ corner button | Restart the run |
| `Esc` / ✕ corner button | Pause menu |
| `Enter` | Watch the replay from the crash screen |
| Gamepad (standard layout) | A / R2 / D-pad up = thrust, left stick X or D-pad = rotate, Start or Y = restart |

**Touch** (phones and tablets) uses two floating controls that appear
wherever your fingers land:

- **Split controls** (default): a touch on the left half of the screen
  spawns a **throttle button** under the finger (hold = full throttle); a
  touch on the right half spawns an **attitude stick** — push in a
  direction and the ship rotates the short way to point its nose there
  (a nudge trims, a rim push flips). Settings → *Swap control sides*
  mirrors the layout for left-handed play.
- **One-handed** (Settings → *Split controls* off): the stick does both —
  holding it fires the engine, its direction points the nose, release to
  coast.
- *Invert stick* reverses the commanded direction, like pulling back on a
  flight stick.

How the controls feel is governed by a small set of constants — see
[`docs/control-tuning.md`](docs/control-tuning.md) for the knob reference
and preset recipes, and [`docs/touch-input.md`](docs/touch-input.md) for
how a touch event reaches the stick (and the trap on the way).

## Flying

- **Landing** = both feet on a pad deck, slow (under 1 m/s), not turning,
  held for 0.4 s — a green settle ring fills while the landing registers.
  Parked ships refuel and repair.
- **Impacts** are graduated: a gentle touch is free, a scrape damages the
  hull in proportion to the impact, a hard hit (or a scrape on an empty
  hull) destroys the ship. Running out of fuel ends the run a few seconds
  later.
- **Replays**: every run is recorded as inputs + periodic keyframes and
  re-simulated for playback, with play/pause, scrubbing and ¼×–4× speed
  (in-canvas: `Space` pause, `←`/`→` step, `S` speed). Watch your own
  crash, or any board entry's run.
- The in-game **Flight manual** (About → Flight manual) spells out the
  rules in player language.

## Levels

Levels are plain-text `key = value` files in [`levels/`](levels/), fetched
at runtime and listed in `levels/manifest.json` — adding a level means
adding a file and a manifest entry, no wasm rebuild. Three families ship,
each in three modes:

| World | Distance | Sprint (60 s clock) | Dash (1,000 m time trial) |
|-------|----------|---------------------|---------------------------|
| **The Expanse** — one long winding tunnel full of boulders | ✓ | ✓ | ✓ |
| **The Glide** — pure cave flying, no boulders | ✓ | ✓ | ✓ |
| **The Flux** — an endless cave that reshuffles itself on every attempt | ✓ | ✓ | ✓ |

Plus **The Hollows**, a hand-drawn map: five chambers, five pads, visit
them all against the clock.

Three scoring modes: **distance** (farthest |x| reached, metres),
**time** (visit every pad, or reach the finish pad — fastest wins) and
**pads** (+100 per first landing; the built-in fallback world). Level files
can also tune gravity, thrust, tank, hull and refuel rate per level.
Hand-drawn worlds are polygons of solid rock, authored in `editor.html`
(a standalone page, not yet linked from the game's menus).

See "Levels" in [`CLAUDE.md`](CLAUDE.md) for the full key reference.

## High scores, replays and the ghost

Boards are **global** (today / this week / all time, per level). Every
submission carries its replay, which the score server
**re-simulates with the same physics crate** before the score can reach a
board — the boards stay physics-true. The level's record run is fetched on
load and raced as a **ghost**; board rows tagged `v1` / `v2` / … name the
rulebook version each run was flown under (old runs keep replaying under
their own rules).

Offline or without a backend config the game plays identically — no
boards, no ghost, a session-only best.

## Project layout

| Path | What |
|------|------|
| `src/` | The game: frame loop, input, rendering, HUD, replay playback, wasm ↔ JS bridge |
| `sim-core/` | `pegasus-sim` — the deterministic simulation (physics, world generation, replay format), also compiled by the server-side score verifier |
| `index.html` | Web wrapper: HTML menus, settings, boards, gamepad polling, analytics |
| `levels/` | Runtime level data |
| `editor.html` | Standalone hand-drawn level editor |
| `ios/`, `android/` | Native app shells — see their READMEs |
| `tools/` | Build recipe, version script, changelog and license generators |
| `docs/` | Control tuning, touch input, multiplayer design brief |
| `tests/touch-e2e/` | Headless browser regression test for the touch stick |

[`CLAUDE.md`](CLAUDE.md) is the detailed architecture and conventions
reference.

## Development

### Prerequisites

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # rustup
```

The toolchain is pinned in `rust-toolchain.toml` (rustup installs it, wasm
target included, on first use). `python3` is needed for the changelog and
license generators.

### Build & test

```bash
cargo build                        # native sanity build (silent — audio is wasm-only)
cargo test --workspace             # unit tests (--workspace includes sim-core)
tools/build-wasm.sh pegasus.wasm   # the deploy's exact wasm into the repo root
```

`tools/build-wasm.sh` is the one wasm recipe — pinned toolchain, pinned
`wasm-opt` (sha256-verified download), paths remapped — so its output is
byte-identical to what the website serves for the same commit.
`tools/build-site.sh` assembles the whole `site/` directory the same way.

### Run locally

```bash
python3 -m http.server 8080        # then open http://localhost:8080
```

The page loads `pegasus.wasm` from the repo root. With no `config.json`
the online layer is off (no boards, no ghost). For phone testing, tunnel
it with `ngrok http 8080` — some browser features (wake lock, share sheet)
need a secure context.

### Touch regression test

```bash
cd tests/touch-e2e && npm ci && npm test
```

Runs headless Chromium against the built wasm (`CHROMIUM_PATH=…` reuses a
preinstalled browser). CI runs it on every PR, together with clippy, the
unit tests, the wasm build and a twice-build reproducibility check.

## Deployment

Every push to `main` deploys the site: `deploy.yml` builds and syncs the
`gh-pages` state branch, `publish-pages.yml` snapshots that branch to
GitHub Pages. Every pull request gets a preview at
`https://pegasusmoonlander.com/pr-<n>/` (posted as a sticky PR comment,
torn down on close), and a `test-apk` label builds an installable Android
test build for the PR.

**Versions** come from annotated `vMAJOR.MINOR.PATCH` tags via
`tools/version.sh` — `1.3.0+14` on the web (tag + commits since), the
bare tag as the store apps' marketing version. Pushing a tag builds and
uploads both store apps at that commit; see "Versioning" in `CLAUDE.md`
for the release cycle.

### Verifying a build

Every `main` deploy signs a provenance attestation for the served wasm and
page, and the build is reproducible from the commit:

```bash
curl -fsSO https://pegasusmoonlander.com/pegasus.wasm
gh attestation verify pegasus.wasm --repo dannyrhubarb/pegasus
```

The same command on a local `tools/build-wasm.sh` output verifies too, as
long as the bytes match — attestations are looked up by digest.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Commits follow Conventional
Commits, and every player-visible change carries a `Whats-new:` trailer
that becomes an entry on the in-game What's New page.

## License

Pegasus is licensed under [GPL-3.0-or-later](LICENSE). Contributions are
accepted under the terms of the [Contributor License Agreement](CLA.md).

Third-party components are attributed in
[third-party-licenses.html](third-party-licenses.html) (also linked from
the in-game About screen). The page is generated — after changing
dependencies, regenerate it with `python3 tools/gen-third-party-licenses.py`.
