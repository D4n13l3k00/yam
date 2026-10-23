# Развёртывание

## Installer

Installer скачивает последний GitHub Release, проверяет `SHA256SUMS`, устанавливает `yamd` и `yam` в `/data/quazar/yam` и запускает daemon через systemd, если он доступен.

```sh
curl -fsSL https://raw.githubusercontent.com/nyakokitsu/yam/main/install.sh | sudo sh
curl -fsSL https://raw.githubusercontent.com/nyakokitsu/yam/main/install.sh | sudo sh -s -- 0.1.1 /data/quazar/custom-yam
```

Первый аргумент — версия (по умолчанию `latest`), второй — каталог установки. Скрипт создаёт `bin/yamd`, `bin/yam`, `data/` и `run.sh`.

## Windows

В PowerShell скачайте installer из репозитория и запустите его. По умолчанию он получает последний GitHub Release, проверяет `SHA256SUMS` и устанавливает CLI в `%LOCALAPPDATA%\YAM\bin`:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/nyakokitsu/yam/main/install.ps1 -OutFile install.ps1
.\install.ps1
```

Для конкретной версии: `.\install.ps1 -Version 0.1.1`. Windows CLI управляет daemon на Linux-колонке через `--url http://<speaker-ip>:8099`.

## Развёртывание через ADB

Для подключённого ADB-устройства:

```sh
adb -s <ip>:5555 shell 'mkdir -p /data/quazar/yam/bin /data/quazar/yam/data'
adb -s <ip>:5555 push target/aarch64-unknown-linux-musl/release/yamd /data/quazar/yam/bin/yamd
adb -s <ip>:5555 shell 'chmod 755 /data/quazar/yam/bin/yamd'
adb -s <ip>:5555 shell 'nohup /data/quazar/yam/bin/yamd --data-dir /data/quazar/yam/data >/data/quazar/yam/yamd.log 2>&1 &'
```

Для известной sysfs-раскладки Station Lite/Micro:

```toml
allow_plugin_binaries = false

[led]
profile = "micro"
count = 3
sysfs_root = "/sys/class/leds"
```

Сохраните настройки в `/data/quazar/yam/data/settings.toml` до первого запуска.

## Эксплуатация

```sh
adb -s <ip>:5555 shell 'tail -f /data/quazar/yam/yamd.log'
adb -s <ip>:5555 forward tcp:18099 tcp:8099
curl http://127.0.0.1:18099/api/device
curl -X POST http://127.0.0.1:18099/api/led/off
```

Web UI не содержит аутентификации. Открывайте его только в доверенной LAN или используйте reverse proxy с аутентификацией.

## Отключение Web UI и API

В `settings.toml` задайте секцию `web`:

```toml
[web]
ui = false
api = false
```

При выключении обоих значений `yamd` не открывает HTTP-порт, но продолжает запускать включённые service-пакеты. Можно оставить только API для управления через CLI:

```toml
[web]
ui = false
api = true
```

Комбинация `ui = true` и `api = false` недопустима.
