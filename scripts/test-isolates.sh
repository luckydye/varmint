#!/usr/bin/env bash
# End-to-end test of the varmint CLI against a real VM.
# usage: scripts/test-isolates.sh [path/to/varmint]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VARMINT="${1:-$ROOT/dist/Varmint.app/Contents/MacOS/varmint}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/varmint-test.XXXXXX")"
export VARMINT_HOME="$WORK/home"
vm=""

cleanup() {
    [ -z "$vm" ] || "$VARMINT" stop "$vm" >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# Warm the base image so the timed run measures VM creation only.
"$VARMINT" stop "$("$VARMINT" run)"

start=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
vm=$("$VARMINT" run)
elapsed=$(perl -MTime::HiRes=time -e "printf '%.3f', time - $start")
echo "created and booted $vm in ${elapsed}s"
perl -e "exit($elapsed < 2 ? 0 : 1)" || fail "boot took ${elapsed}s, expected under 2s"

[ "$("$VARMINT" exec "$vm" -- whoami)" = varmint ] || fail "exec whoami"
[ "$(echo hello | "$VARMINT" exec "$vm" -- cat)" = hello ] || fail "exec stdin"
set +e; "$VARMINT" exec "$vm" -- sh -c 'exit 7'; code=$?; set -e
[ "$code" -eq 7 ] || fail "exit code: got $code"
[ "$("$VARMINT" ssh "$vm" -- echo ssh-ok)" = ssh-ok ] || fail "ssh"
"$VARMINT" ls | grep -q "^$vm .*running" || fail "ls"

# The desktop comes up after the agent; wait for a configured scanout.
for _ in $(seq 100); do
    "$VARMINT" screenshot "$vm" "$WORK/shot.png" >/dev/null 2>&1 && break
    sleep 0.1
done
file "$WORK/shot.png" | grep -q "PNG image data, 1280 x 800" || fail "screenshot"
"$VARMINT" click "$vm" 640 400
"$VARMINT" type "$vm" "Hello, World!"
"$VARMINT" key "$vm" ctrl+a

"$VARMINT" stop "$vm"
[ ! -d "$VARMINT_HOME/vms/$vm" ] || fail "ephemeral VM was not deleted"
vm=""

# Host directories shared over virtio-fs.
mkdir -p "$WORK/share/sub" "$WORK/readonly"
echo from-host > "$WORK/share/sub/file.txt"
echo locked > "$WORK/readonly/file.txt"
ln -s / "$WORK/share/escape"
vm=$("$VARMINT" run --mount "$WORK/share:/mnt/share" --mount "$WORK/readonly:ro:ro")
[ "$("$VARMINT" exec "$vm" -- cat /mnt/share/sub/file.txt)" = from-host ] || fail "read from share"
"$VARMINT" exec "$vm" -- sh -c 'echo from-guest > /mnt/share/new.txt && mkdir /mnt/share/dir && mv /mnt/share/new.txt /mnt/share/dir/moved.txt'
[ "$(cat "$WORK/share/dir/moved.txt")" = from-guest ] || fail "write and rename into share"
[ "$("$VARMINT" exec "$vm" -- stat -c %U /mnt/share/sub/file.txt)" = varmint ] || fail "share ownership"
[ "$("$VARMINT" exec "$vm" -- readlink /mnt/share/escape)" = / ] || fail "symlink in share"
rmdir_output=$("$VARMINT" exec "$vm" -- sh -c 'rmdir /mnt/share/sub 2>&1 || true')
[[ "$rmdir_output" == *"Directory not empty"* ]] || fail "Linux errno for ENOTEMPTY: $rmdir_output"
[ "$("$VARMINT" exec "$vm" -- cat ro/file.txt)" = locked ] || fail "read from read-only share"
if "$VARMINT" exec "$vm" -- sh -c 'echo x > ro/file.txt' 2>/dev/null; then fail "read-only share accepted a write"; fi
[ "$(cat "$WORK/readonly/file.txt")" = locked ] || fail "read-only share was modified"
"$VARMINT" stop "$vm"
vm=""

vm=$("$VARMINT" run persisted --persist)
"$VARMINT" exec "$vm" -- sh -c 'echo kept > ~/marker'
"$VARMINT" stop "$vm"
"$VARMINT" run persisted >/dev/null
[ "$("$VARMINT" exec "$vm" -- cat marker)" = kept ] || fail "persistent disk"
"$VARMINT" rm "$vm"
vm=""

echo PASS
