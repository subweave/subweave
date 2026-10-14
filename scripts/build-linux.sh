#!/bin/sh
# Static musl binaries for Linux boxes, x86_64 and aarch64, built on this machine with
# cargo-zigbuild from the isolated toolchain in ~/.cache/subweave-toolchain.
#
# Out: ~/.cache/subweave/bin/<version>/<target>/subweave (what `--on` uploads) and
#      dist/subweave-<target> with dist/subweave-<target>.sha256 (what a release holds).
set -eu

cd "$(dirname "$0")/.."
T="$HOME/.cache/subweave-toolchain"
if [ ! -x "$T/cargo/bin/cargo-zigbuild" ]; then
  echo "no toolchain at $T: install rustup there with the musl targets and cargo-zigbuild" >&2
  exit 1
fi
command -v zig >/dev/null || { echo "zig is not installed: brew install zig" >&2; exit 1; }

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
mkdir -p dist
for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  echo "building subweave $version for $target"
  # A target directory of its own: this toolchain is not the one that builds for macOS.
  # zig's linker reports rustc's -O1 as deprecated on every link; it changes nothing.
  RUSTUP_HOME="$T/rustup" CARGO_HOME="$T/cargo" CARGO_TARGET_DIR=target/linux \
    RUSTFLAGS="-A linker-messages" \
    "$T/cargo/bin/cargo" zigbuild --release --target "$target"
  bin="target/linux/$target/release/subweave"
  cache="$HOME/.cache/subweave/bin/$version/$target"
  mkdir -p "$cache"
  cp "$bin" "$cache/subweave"
  cp "$bin" "dist/subweave-$target"
  (cd dist && shasum -a 256 "subweave-$target" > "subweave-$target.sha256")
  echo "  $cache/subweave"
  echo "  dist/subweave-$target"
done
