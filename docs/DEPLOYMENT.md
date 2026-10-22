# Deployment

## Installer

The installer receives a URL to a prebuilt binary and an optional destination. Its default destination is `/data/quazar/yaff`.

```sh
curl -fsSL https://<host>/yaff/install.sh | sudo sh -s -- https://<host>/yaff/yaff-aarch64-linux-musl
curl -fsSL https://<host>/yaff/install.sh | sudo sh -s -- https://<host>/yaff/yaff-aarch64-linux-musl /data/quazar/custom-yaff
```

It creates `bin/yaff`, `data/` and `run.sh`. On systems with systemd it also installs and starts `yaff.service`.

## ADB deployment

For the connected ADB device:

```sh
adb -s <ip>:5555 shell 'mkdir -p /data/quazar/yaff/bin /data/quazar/yaff/data'
adb -s <ip>:5555 push target/aarch64-unknown-linux-musl/release/yaff /data/quazar/yaff/bin/yaff
adb -s <ip>:5555 shell 'chmod 755 /data/quazar/yaff/bin/yaff'
adb -s <ip>:5555 shell 'nohup /data/quazar/yaff/bin/yaff --data-dir /data/quazar/yaff/data >/data/quazar/yaff/yaff.log 2>&1 &'
```

For the known Station Lite/Micro sysfs layout:

```toml
allow_plugin_binaries = false

[led]
profile = "micro"
count = 3
sysfs_root = "/sys/class/leds"
```

Save it as `/data/quazar/yaff/data/settings.toml` before the first start.

## Operations

```sh
adb -s <ip>:5555 shell 'tail -f /data/quazar/yaff/yaff.log'
adb -s <ip>:5555 forward tcp:18099 tcp:8099
curl http://127.0.0.1:18099/api/device
curl -X POST http://127.0.0.1:18099/api/led/off
```

The Web UI is unauthenticated. Bind it to a trusted LAN only, or use an authenticated reverse proxy.
