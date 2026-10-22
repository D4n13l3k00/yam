# Развёртывание

## Installer

Installer принимает URL готового бинарника и необязательный каталог установки. Каталог по умолчанию — `/data/quazar/yaff`.

```sh
curl -fsSL https://<host>/yaff/install.sh | sudo sh -s -- https://<host>/yaff/yaff-aarch64-linux-musl
curl -fsSL https://<host>/yaff/install.sh | sudo sh -s -- https://<host>/yaff/yaff-aarch64-linux-musl /data/quazar/custom-yaff
```

Скрипт создаёт `bin/yaff`, `data/` и `run.sh`. На системах с systemd он также устанавливает и запускает `yaff.service`.

## Развёртывание через ADB

Для подключённого ADB-устройства:

```sh
adb -s <ip>:5555 shell 'mkdir -p /data/quazar/yaff/bin /data/quazar/yaff/data'
adb -s <ip>:5555 push target/aarch64-unknown-linux-musl/release/yaff /data/quazar/yaff/bin/yaff
adb -s <ip>:5555 shell 'chmod 755 /data/quazar/yaff/bin/yaff'
adb -s <ip>:5555 shell 'nohup /data/quazar/yaff/bin/yaff --data-dir /data/quazar/yaff/data >/data/quazar/yaff/yaff.log 2>&1 &'
```

Для известной sysfs-раскладки Station Lite/Micro:

```toml
allow_plugin_binaries = false

[led]
profile = "micro"
count = 3
sysfs_root = "/sys/class/leds"
```

Сохраните настройки в `/data/quazar/yaff/data/settings.toml` до первого запуска.

## Эксплуатация

```sh
adb -s <ip>:5555 shell 'tail -f /data/quazar/yaff/yaff.log'
adb -s <ip>:5555 forward tcp:18099 tcp:8099
curl http://127.0.0.1:18099/api/device
curl -X POST http://127.0.0.1:18099/api/led/off
```

Web UI не содержит аутентификации. Открывайте его только в доверенной LAN или используйте reverse proxy с аутентификацией.
