#!/bin/sh
set -eu

URL=${1:-${YAM_BINARY_URL:-}}
PREFIX=${2:-${YAM_PREFIX:-/data/quazar/yam}}
CLI_URL=${3:-${YAM_CLI_URL:-}}
SERVICE_NAME=yam.service

if [ "$(id -u)" -ne 0 ]; then
  echo 'Run as root (use sudo).' >&2
  exit 1
fi
if [ -z "$URL" ]; then
  echo 'Binary URL is required: install.sh https://host/yamd-aarch64-linux-musl' >&2
  exit 2
fi
case "$(uname -m)" in
  aarch64|armv7l|armv6l|armhf) ;;
  *) echo "Unsupported architecture $(uname -m); YAM installer requires ARM." >&2; exit 3 ;;
esac
case "$URL" in https://*|http://*) ;; *) echo 'Refusing a non-HTTP(S) binary URL.' >&2; exit 2;; esac

mkdir -p "$PREFIX/bin" "$PREFIX/data"
tmp=$(mktemp "${TMPDIR:-/tmp}/yam.XXXXXX")
trap 'rm -f "$tmp"' EXIT HUP INT TERM
if command -v curl >/dev/null 2>&1; then
  curl -fL --proto '=https,http' --tlsv1.2 "$URL" -o "$tmp"
elif command -v wget >/dev/null 2>&1; then
  wget -O "$tmp" "$URL"
else
  echo 'curl or wget is required.' >&2; exit 4
fi
install -m 0755 "$tmp" "$PREFIX/bin/yamd"
if [ -n "$CLI_URL" ]; then
  if command -v curl >/dev/null 2>&1; then
    curl -fL --proto '=https,http' --tlsv1.2 "$CLI_URL" -o "$tmp"
  else
    wget -O "$tmp" "$CLI_URL"
  fi
  install -m 0755 "$tmp" "$PREFIX/bin/yam"
fi
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
echo "YAM installed in $PREFIX. Open http://$(hostname -I 2>/dev/null | awk '{print $1}'):8099"
