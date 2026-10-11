#!/usr/bin/env bash
# Installs varmint as PREFIX/bin/varmint with its runtime (helpers, libraries,
# kernel and base image) in PREFIX/share/varmint.
# usage: scripts/install.sh [PREFIX]   (default: ~/.local)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${1:-$HOME/.local}"
CONTENTS="$ROOT/dist/Varmint.app/Contents"
SHARE="$PREFIX/share/varmint"
BIN="$PREFIX/bin/varmint"

[ -x "$CONTENTS/MacOS/varmint" ] || { echo "error: $CONTENTS is missing; run: make bundle" >&2; exit 1; }

staging="$PREFIX/share/.varmint.partial.$$"
binary="$PREFIX/bin/.varmint.partial.$$"
trap 'rm -rf "$staging" "$binary"' EXIT
mkdir -p "$staging/Resources" "$PREFIX/bin"

# -c clones on APFS, so the base image costs no extra space or time.
cp -Rc "$CONTENTS/Helpers" "$CONTENTS/Frameworks" "$staging/"
cp -Rc "$CONTENTS/Resources/kernel" "$CONTENTS/Resources/runtime" "$staging/Resources/"

cp "$CONTENTS/MacOS/varmint" "$binary"
install_name_tool -add_rpath @executable_path/../share/varmint/Frameworks "$binary"
codesign --force --sign - --timestamp=none --entitlements "$ROOT/runtime/entitlements.plist" "$binary"

rm -rf "$SHARE"
mv "$staging" "$SHARE"
mv "$binary" "$BIN"
echo "installed $BIN"
