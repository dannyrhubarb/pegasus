# rapier-test

A 2D physics demo using [Rapier](https://rapier.rs) and [macroquad](https://macroquad.rs), compiled to WebAssembly.

## Controls

| Input | Action |
|-------|--------|
| Click / Down arrow | Thrust in the direction the box is pointing |
| Left / Right arrow | Rotate |
| R | Reset |

## Development

### Build

```bash
cargo build --release --target wasm32-unknown-unknown && \
  cp target/wasm32-unknown-unknown/release/rapier-test.wasm rapier-test.wasm
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

## First-time setup

```bash
rustup target add wasm32-unknown-unknown
brew install ngrok  # optional, for iOS testing
```
