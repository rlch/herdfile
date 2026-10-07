#!/bin/sh
# Plugin build step: fetch the release binary for this machine into ./bin,
# falling back to building from source with cargo.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
repo="rlch/herdfile"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/herdr-plugin.toml" | head -n1)

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-gnu ;;
  *) os="" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) arch="" ;;
esac

mkdir -p "$here/bin"
if [ -n "$os" ] && [ -n "$arch" ] && command -v curl >/dev/null 2>&1; then
  target="$arch-$os"
  url="https://github.com/$repo/releases/download/v$version/herdfile-$target.tar.gz"
  if curl -fsSL "$url" -o "$here/bin/herdfile.tar.gz" 2>/dev/null; then
    tar -xzf "$here/bin/herdfile.tar.gz" -C "$here/bin"
    rm -f "$here/bin/herdfile.tar.gz"
    chmod +x "$here/bin/herdfile"
    echo "herdfile: installed release v$version for $target"
    exit 0
  fi
  echo "herdfile: no release binary at $url; building from source"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "herdfile: no release binary for this machine and no cargo to build one" >&2
  exit 1
fi
cargo install --locked --path "$here" --root "$here" --force
echo "herdfile: built from source"
