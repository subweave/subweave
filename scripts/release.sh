#!/bin/sh
# Builds the Linux binaries, then publishes them as the GitHub release v<version>.
set -eu

cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
rm -rf dist
scripts/build-linux.sh

echo
echo "gh release create v$version dist/* --title v$version --notes \"subweave $version\""
printf 'run it? [y/N] '
read -r answer
case "$answer" in
  y | Y | yes) ;;
  *) echo "nothing published"; exit 1 ;;
esac
gh release create "v$version" dist/* --title "v$version" --notes "subweave $version"
