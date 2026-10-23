#!/bin/sh
set -eu

VERSION=${1:-${YAM_VERSION:-latest}}
BIN_DIR=${2:-${YAM_BIN_DIR:-$HOME/.local/bin}}
REPOSITORY=https://github.com/nyakokitsu/yam
DEVICE_PREFIX=/data/quazar/yam

case "$(uname -m)" in
  x86_64|amd64) ;;
  *) echo "Unsupported host architecture $(uname -m); install the CLI on an x86_64 computer." >&2; exit 3 ;;
esac

case "$VERSION" in
  latest) RELEASE_URL="$REPOSITORY/releases/latest/download" ;;
  v*) RELEASE_URL="$REPOSITORY/releases/download/$VERSION" ;;
  *) RELEASE_URL="$REPOSITORY/releases/download/v$VERSION" ;;
esac

tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/yam.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
download() {
  url=$1 output=$2
  if command -v curl >/dev/null 2>&1; then
    curl -fL --proto '=https,http' --tlsv1.2 "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$output" "$url"
  else
    echo 'curl or wget is required.' >&2; exit 4
  fi
}
verify() {
  file=$1
  expected=$(awk -v name="$(basename "$file")" '$2 == name {print $1}' "$tmpdir/SHA256SUMS")
  [ -n "$expected" ] || { echo "No checksum for $(basename "$file")" >&2; exit 5; }
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$file" | awk '{print $1}')
  elif command -v openssl >/dev/null 2>&1; then
    actual=$(openssl dgst -sha256 "$file" | awk '{print $NF}')
  else
    echo 'sha256sum or openssl is required.' >&2; exit 5
  fi
  [ "$actual" = "$expected" ] || { echo "Checksum mismatch for $(basename "$file")" >&2; exit 5; }
}

command -v adb >/dev/null 2>&1 || {
  echo 'adb is required to deploy yamd. Install Android Platform Tools first.' >&2; exit 4;
}

find_device() {
  adb devices | awk 'NR > 1 && $2 == "device" {print $1}' | while IFS= read -r serial; do
    arch=$(adb -s "$serial" shell uname -m 2>/dev/null | tr -d '\r') || continue
    case "$arch" in aarch64|arm64) printf '%s\n' "$serial"; return 0;; esac
  done
  return 1
}

DEVICE=$(find_device || true)
if [ -z "$DEVICE" ]; then
  printf 'Yandex Station IP address: ' >&2
  IFS= read -r ip </dev/tty
  [ -n "$ip" ] || { echo 'IP address is required.' >&2; exit 6; }
  DEVICE="$ip:5555"
  adb connect "$DEVICE" >&2
  sleep 1
  adb -s "$DEVICE" get-state >/dev/null
fi

download "$RELEASE_URL/SHA256SUMS" "$tmpdir/SHA256SUMS"
download "$RELEASE_URL/yam-x86_64-unknown-linux-musl" "$tmpdir/yam-x86_64-unknown-linux-musl"
download "$RELEASE_URL/yamd-aarch64-linux-musl" "$tmpdir/yamd-aarch64-linux-musl"
verify "$tmpdir/yam-x86_64-unknown-linux-musl"
verify "$tmpdir/yamd-aarch64-linux-musl"

mkdir -p "$BIN_DIR"
install -m 0755 "$tmpdir/yam-x86_64-unknown-linux-musl" "$BIN_DIR/yam"
adb -s "$DEVICE" shell "mkdir -p $DEVICE_PREFIX/bin $DEVICE_PREFIX/data"
adb -s "$DEVICE" push "$tmpdir/yamd-aarch64-linux-musl" "$DEVICE_PREFIX/bin/yamd.new" >/dev/null
adb -s "$DEVICE" shell "chmod 0755 $DEVICE_PREFIX/bin/yamd.new; if [ -f $DEVICE_PREFIX/yamd.pid ] && kill -0 \$(cat $DEVICE_PREFIX/yamd.pid) 2>/dev/null; then kill \$(cat $DEVICE_PREFIX/yamd.pid); fi; mv $DEVICE_PREFIX/bin/yamd.new $DEVICE_PREFIX/bin/yamd; start-stop-daemon --start --background --make-pidfile --pidfile $DEVICE_PREFIX/yamd.pid --startas $DEVICE_PREFIX/bin/yamd -- --data-dir $DEVICE_PREFIX/data"

echo "YAM $VERSION CLI installed in $BIN_DIR/yam"
echo "yamd deployed to $DEVICE ($DEVICE_PREFIX). Open http://${DEVICE%%:*}:8099"
