# Разработка плагинов

## Структура пакета

`.yap` — ZIP-архив. В его корне должен находиться `plugin.toml`.

```text
my-plugin.yap
├── plugin.toml
├── main.lua
├── patterns/
│   └── pulse.led
├── sounds/
│   └── hello.wav
└── bin/
    └── helper
```

```toml
id = "my-plugin"
name = "My plugin"
version = "1.0.0"
entry = "main.lua"
description = "Example package"
permissions = ["led"]

[compatibility]
models = ["yandexmicro"]
features = ["led.ring.rgb", "led.ring.3"]
```

Создание архива:

```sh
cd my-plugin
zip -r ../my-plugin.yap plugin.toml main.lua patterns sounds bin
```

## Lua-окружение

В Lua доступны `table`, `string`, `math` и `utf8`. Библиотеки `os`, `io`, `package`, debug и сеть недоступны. Глобальные объекты:

- `event` содержит `manual` при запуске через Web UI.
- `device.model`, `device.architecture`, `device.led_count` и `device.features` описывают колонку.
- `alice` содержит только API, разрешённые permissions пакета.

## Permissions и API

| Permission | API | Требование |
| --- | --- | --- |
| `led` | `alice.set(colors)`, `alice.off()`, `alice.play(path)`, `alice.sleep(ms)` | RGB LED-профиль |
| `volume` | `alice.volume.set(percent)`, `alice.volume.get()` | `audio.volume.alsa` |
| `audio` | `alice.audio.play(relative_path)` | `audio.play.aplay` |
| `bluetooth` | `alice.bluetooth.status()`, `power(bool)`, `scan(bool)`, `pair(mac)`, `connect(mac)`, `disconnect(mac)`, `remove(mac)` | `bluetooth.bluez` |
| `display` | `alice.display.backlights()`, `set_backlight(name, value)` | `display.backlight` |
| `system` | `alice.system.hostname()`, `uptime()`, `meminfo()` | нет |
| `exec` | `alice.exec(program, args)` | runtime-настройка `allow_plugin_binaries = true` |

`alice.set` принимает ровно `device.led_count` строк формата `#RRGGBB`. `alice.sleep` принимает 0–60000 мс и является точкой отмены анимации.

## Совместимость

`models` проверяется точным совпадением. Для `features` должны присутствовать все перечисленные возможности. При несовместимости YAFF сохраняет пакет, но блокирует запуск и показывает причину в Web UI.

По возможности указывайте требования к возможностям, а не к модели. Например, RGB-анимация должна требовать `led.ring.rgb`; точный размер кольца нужен только если от него зависит раскладка кадров.

## Жизненный цикл

Действие запуска асинхронно. Повторный запуск отменяет предыдущее выполнение этого же плагина перед новым стартом. Остановка отправляет отмену и выключает LED-контроллер runtime. В каждом кадре анимации вызывайте `alice.sleep` или `alice.set`.
