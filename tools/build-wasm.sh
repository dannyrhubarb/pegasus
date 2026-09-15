#!/usr/bin/env bash
# Build and optimize the game wasm REPRODUCIBLY (#214 step 5).
#
#   tools/build-wasm.sh <output.wasm>
#
# The same commit must yield the same bytes on every machine, so every
# input is pinned here or next to here:
#   - rustc / cargo: rust-toolchain.toml (rustup installs it on first use);
#     --locked refuses a Cargo.lock that does not match Cargo.toml.
#   - absolute paths: the checkout and the cargo registry are remapped to
#     fixed names, so panic-location strings (the only place a path reaches
#     the binary — the release profile carries no debug info) do not encode
#     WHERE the build ran. --config merges with .cargo/config.toml (arrays
#     join), so the wasm link-arg there still applies.
#   - wasm-opt: a pinned Binaryen release, verified by sha256 and cached
#     under target/tools/ (target/ is in the CI cargo cache). Never the
#     distro package: apt/brew hand out whatever version the image has,
#     and different Binaryen versions emit different bytes.
# CI proves the property on every PR: ci.yml's `reproducible` job builds
# two checkouts at different paths and diffs the whole site/ tree.
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:?usage: $0 <output.wasm>}"

BINARYEN_VERSION=version_132
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)   asset=x86_64-linux;  sha=195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572 ;;
  Linux-aarch64)  asset=aarch64-linux; sha=c58562417836c5d0493d89bdefc434933bdc097db641b483df86bcfa557a107f ;;
  Darwin-arm64)   asset=arm64-macos;   sha=98aad827847af7ef990ed7098d885725c8e5b5aae75073403635617ae4e259aa ;;
  Darwin-x86_64)  asset=x86_64-macos;  sha=40c3de90bb3766bd0282a895e139a6f50253dba49b4f5bb89e66faca162d832e ;;
  *) echo "build-wasm.sh: no pinned Binaryen build for $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

tools_dir="target/tools"
wasm_opt="$tools_dir/binaryen-$BINARYEN_VERSION/bin/wasm-opt"
if [ ! -x "$wasm_opt" ]; then
  name="binaryen-$BINARYEN_VERSION-$asset.tar.gz"
  tgz="$tools_dir/$name"
  mkdir -p "$tools_dir"
  echo "build-wasm.sh: fetching Binaryen $BINARYEN_VERSION ($asset)"
  curl -fsSL --retry 3 -o "$tgz" \
    "https://github.com/WebAssembly/binaryen/releases/download/$BINARYEN_VERSION/$name"
  if command -v sha256sum >/dev/null; then
    echo "$sha  $tgz" | sha256sum -c - >/dev/null
  else
    echo "$sha  $tgz" | shasum -a 256 -c - >/dev/null # macOS
  fi
  # Only wasm-opt and the shared library it links; the rest of the release
  # (a dozen tools, unit tests) would just bloat the CI cache.
  tar xzf "$tgz" -C "$tools_dir" \
    "binaryen-$BINARYEN_VERSION/bin/wasm-opt" "binaryen-$BINARYEN_VERSION/lib"
  rm -f "$tgz"
fi

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
cargo build --release --locked --target wasm32-unknown-unknown \
  --config "target.wasm32-unknown-unknown.rustflags=[\"--remap-path-prefix=$PWD=/pegasus\", \"--remap-path-prefix=$cargo_home=/cargo\"]"

mkdir -p "$(dirname "$out")"
"$wasm_opt" -Oz -o "$out" target/wasm32-unknown-unknown/release/pegasus.wasm
if command -v sha256sum >/dev/null; then sha256sum "$out"; else shasum -a 256 "$out"; fi
