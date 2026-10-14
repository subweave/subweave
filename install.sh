#!/bin/sh
# Installs subweave on a Linux box:
#   curl -fsSL https://subweave.com/install.sh | sh
# SUBWEAVE_VERSION=0.1.0 picks a release; the newest by default.
set -eu

repo="subweave/subweave"
dest="/usr/local/bin/subweave"

fail() {
  echo "subweave install: $*" >&2
  exit 1
}

[ "$(uname -s)" = Linux ] || fail "this installs on Linux; on macOS: brew install subweave"
case "$(uname -m)" in
  x86_64 | amd64) target=x86_64-unknown-linux-musl ;;
  aarch64 | arm64) target=aarch64-unknown-linux-musl ;;
  *) fail "no build for $(uname -m): subweave runs on x86_64 and aarch64" ;;
esac

if command -v curl >/dev/null; then
  get() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null; then
  get() { wget -qO "$2" "$1"; }
else
  fail "needs curl or wget"
fi

if [ -n "${SUBWEAVE_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/v$SUBWEAVE_VERSION"
else
  base="https://github.com/$repo/releases/latest/download"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "downloading subweave-$target"
get "$base/subweave-$target" "$tmp/subweave" || fail "download failed: $base/subweave-$target"
get "$base/subweave-$target.sha256" "$tmp/sum" || fail "download failed: $base/subweave-$target.sha256"

want=$(cut -d ' ' -f 1 "$tmp/sum")
if command -v sha256sum >/dev/null; then
  got=$(sha256sum "$tmp/subweave" | cut -d ' ' -f 1)
else
  got=$(shasum -a 256 "$tmp/subweave" | cut -d ' ' -f 1)
fi
[ "$got" = "$want" ] || fail "sha256 is $got, the release says $want: nothing installed"

sudo=""
if [ "$(id -u)" != 0 ]; then
  command -v sudo >/dev/null || fail "run as root to install to $dest"
  sudo=sudo
fi
$sudo install -m 755 "$tmp/subweave" "$dest"

echo "installed $("$dest" version) at $dest"
echo
echo "next:"
echo "  subweave key openrouter                     # paste an OpenRouter key"
echo "  subweave plan postgres \"postgres 18\"        # check a request; changes nothing"
echo "  subweave run postgres \"postgres 18\"         # run it; starts the daemon under systemd"
