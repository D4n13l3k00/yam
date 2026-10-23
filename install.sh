#!/bin/sh
set -eu

VERSION=${1:-${YAM_VERSION:-latest}}
PREFIX=${2:-${YAM_PREFIX:-/data/quazar/yam}}
SERVICE_NAME=yam.service
REPOSITORY=https://github.com/nyakokitsu/yam

if [ "$(id -u)" -ne 0 ]; then
  echo 'Run as root (use sudo).' >&2
  exit 1
fi
case "$(uname -m)" in
  aarch64|armv7l|armv6l|armhf) ;;
  *) echo "Unsupported architecture $(uname -m); YAM installer requires ARM." >&2; exit 3 ;;
esac

case "$VERSION" in
  latest) RELEASE_URL="$REPOSITORY/releases/latest/download" ;;
  v*) RELEASE_URL="$REPOSITORY/releases/download/$VERSION" ;;
  *) RELEASE_URL="$REPOSITORY/releases/download/v$VERSION" ;;
esac

mkdir -p "$PREFIX/bin" "$PREFIX/data"
tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/yam.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
download() {
  url=$1
  output=$2
  case "$url" in https://*|http://*) ;; *) echo 'Refusing a non-HTTP(S) URL.' >&2; exit 2;; esac
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
    echo 'sha256sum or openssl is required for checksum verification.' >&2; exit 5
  fi
  [ "$actual" = "$expected" ] || { echo "Checksum mismatch for $(basename "$file")" >&2; exit 5; }
}

download "$RELEASE_URL/SHA256SUMS" "$tmpdir/SHA256SUMS"
download "$RELEASE_URL/yamd-aarch64-linux-musl" "$tmpdir/yamd-aarch64-linux-musl"
download "$RELEASE_URL/yam-aarch64-linux-musl" "$tmpdir/yam-aarch64-linux-musl"
verify "$tmpdir/yamd-aarch64-linux-musl"
verify "$tmpdir/yam-aarch64-linux-musl"
install -m 0755 "$tmpdir/yamd-aarch64-linux-musl" "$PREFIX/bin/yamd"
install -m 0755 "$tmpdir/yam-aarch64-linux-musl" "$PREFIX/bin/yam"
cat > "$PREFIX/run.sh" <<EOF
#!/bin/sh
exec "$PREFIX/bin/yamd" --data-dir "$PREFIX/data"
EOF
chmod 0755 "$PREFIX/run.sh"
if command -v systemctl >/dev/null 2>&1; then
cat > /etc/systemd/system/$SERVICE_NAME <<EOF
[Unit]
Description=YAM package daemon
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
User=root
WorkingDirectory=$PREFIX
ExecStart=$PREFIX/bin/yamd --data-dir $PREFIX/data
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
PrivateTmp=true
[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now "$SERVICE_NAME"
else
  echo "systemd not found. Start YAM with: $PREFIX/run.sh" >&2
fi
echo "YAM $VERSION installed in $PREFIX. Open http://$(hostname -I 2>/dev/null | awk '{print $1}'):8099"
