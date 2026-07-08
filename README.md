# Pegasus

Pilot a fragile ship through a procedurally-generated cave system that twists,
narrows, and branches into vertical shafts connecting infinite layers below.
Thread the rock, dodge boulders, and stick the landing on fuel pads before you
run dry — real 2D physics via [Rapier](https://rapier.rs), rendered with
[macroquad](https://macroquad.rs), running straight in the browser via
WebAssembly.

## Controls

| Input | Action |
|-------|--------|
| Click / Down arrow | Thrust in the direction the box is pointing |
| Left / Right arrow | Rotate |
| R | Reset |
| Touch (mobile) | Floating stick: hold = main engine, direction = point the nose (auto-rotates the short way) |

How the controls feel is governed by a small set of constants — see
[`docs/control-tuning.md`](docs/control-tuning.md) for the full knob
reference and preset recipes.

## Levels

Levels are plain-text **data files** in [`levels/`](levels/), fetched at
runtime and selectable from the ⓘ info overlay — adding a level means adding a
`.level` file and listing it in `levels/manifest.json`, no wasm rebuild:

- **The Expanse** — fly as far as you can in either direction; the high score
  is the farthest |x| you reach. No vertical shafts, boulders on, refueling
  pads every ~130 m.
- **The Glide** — The Expanse without the boulders.
- **The Caves** — the original shafted, pad-scoring world, kept as the
  demo/experimentation level.

See the "Levels" section in `CLAUDE.md` for the file format and how the level
parameters ride in every replay recording.

## Development

### Build

```bash
cargo build --release --target wasm32-unknown-unknown && \
  cp target/wasm32-unknown-unknown/release/pegasus.wasm pegasus.wasm
```

### Serve locally

```bash
python3 -m http.server 8080
```

Then open [http://localhost:8080](http://localhost:8080).

### Serve over HTTPS (required for iOS)

```bash
ngrok http 8080
```

Open the `https://` URL ngrok prints on your iPhone.

## Deployment

The project deploys to GitHub Pages automatically. On every push to `main`,
[`.github/workflows/deploy.yml`](.github/workflows/deploy.yml) builds the WASM
from source and syncs the site into the root of the `gh-pages` state branch;
[`.github/workflows/publish-pages.yml`](.github/workflows/publish-pages.yml)
then snapshots that branch and deploys it to Pages.

To enable it, go to **Settings → Pages** in the repository and set
**Source** to **GitHub Actions** (one-time setup). The deploy workflow can also
be run manually from the **Actions** tab via *Run workflow*.

### PR previews

Every pull request gets its own preview deployment — no merge to `main`
required. [`preview-deploy.yml`](.github/workflows/preview-deploy.yml) builds
each PR push and publishes it at

```
https://<owner>.github.io/pegasus/pr-<n>/
```

posting a sticky comment with the link on the PR.
[`preview-teardown.yml`](.github/workflows/preview-teardown.yml) removes the
preview when the PR closes. Previews live in `pr-<n>/` directories on the
`gh-pages` branch alongside the `main` build at the root, so the production
site is never affected.

## First-time setup

```bash
rustup target add wasm32-unknown-unknown
brew install ngrok  # optional, for iOS testing
```
