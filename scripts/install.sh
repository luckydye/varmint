#!/usr/bin/env bash
# Installs varmint as PREFIX/bin/varmint with its runtime (helpers, libraries,
# kernel and base image) in PREFIX/share/varmint.
# usage: scripts/install.sh [PREFIX]   (default: ~/.local)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${1:-$HOME/.local}"
DIST="$ROOT/dist/varmint"
SHARE="$PREFIX/share/varmint"
BIN="$PREFIX/bin/varmint"

[ -x "$DIST/bin/varmint" ] || { echo "error: $DIST is missing; run: make bundle" >&2; exit 1; }

staging="$PREFIX/share/.varmint.partial.$$"
binary="$PREFIX/bin/.varmint.partial.$$"
trap 'rm -rf "$staging" "$binary"' EXIT
mkdir -p "$PREFIX/share" "$PREFIX/bin"

# -c clones on APFS, so the base image costs no extra space or time.
cp -Rc "$DIST/share/varmint" "$staging"
cp -c "$DIST/bin/varmint" "$binary"

rm -rf "$SHARE"
mv "$staging" "$SHARE"
mv "$binary" "$BIN"
echo "installed $BIN"
