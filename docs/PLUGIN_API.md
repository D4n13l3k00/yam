# Plugin development

## Package layout

`.yap` is a ZIP archive. `plugin.toml` must be at its root.

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

Create an archive:

```sh
cd my-plugin
zip -r ../my-plugin.yap plugin.toml main.lua patterns sounds bin
```

## Lua environment

Lua has `table`, `string`, `math` and `utf8`. It does not receive `os`, `io`, `package`, debug or network libraries. Globals:

- `event` is `manual` for a Web UI launch.
- `device.model`, `device.architecture`, `device.led_count` and `device.features` describe the current speaker.
- `alice` contains only APIs approved by the package permissions.

## Permissions and APIs

| Permission | API | Requirement |
| --- | --- | --- |
| `led` | `alice.set(colors)`, `alice.off()`, `alice.play(path)`, `alice.sleep(ms)` | RGB LED profile |
| `volume` | `alice.volume.set(percent)`, `alice.volume.get()` | `audio.volume.alsa` |
| `audio` | `alice.audio.play(relative_path)` | `audio.play.aplay` |
| `bluetooth` | `alice.bluetooth.status()`, `power(bool)`, `scan(bool)`, `pair(mac)`, `connect(mac)`, `disconnect(mac)`, `remove(mac)` | `bluetooth.bluez` |
| `display` | `alice.display.backlights()`, `set_backlight(name, value)` | `display.backlight` |
| `system` | `alice.system.hostname()`, `uptime()`, `meminfo()` | none |
| `exec` | `alice.exec(program, args)` | runtime option `allow_plugin_binaries = true` |

`alice.set` expects exactly `device.led_count` strings in `#RRGGBB` form. `alice.sleep` accepts 0–60000 ms and is the cancellation point for animations.

## Compatibility

`models` is an exact match. `features` is an all-of match. If either condition fails, YAFF keeps the package installed but blocks its start and displays the reason in Web UI.

Prefer feature requirements over model names when possible. For example, an RGB animation should require `led.ring.rgb` and the exact ring size only if its layout depends on it.

## Lifecycle

The Start action is asynchronous. A second Start cancels the previous run for that plugin before launching a new one. Stop sends cancellation and turns the runtime LED controller off. Make animation loops call `alice.sleep` or `alice.set` on every frame.
