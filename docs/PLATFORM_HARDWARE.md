# Аппаратные возможности платформ Yandex

YAM не линкуется с закрытым SDK колонок. Вместо этого он использует только проверяемые в момент запуска Linux-интерфейсы, поэтому пакет переносим между моделями и не обращается к несуществующим model-specific путям.

## Поддерживаемое обнаружение

| Возможность YAM | Источник в platform SDK | Проверка YAM | Feature |
| --- | --- | --- | --- |
| RGB-кольцо Mini 2 / Micro | `yandexmini_2/.../yandex_mini2_led_controller.cpp`, `yandexmicro/.../yandex_micro_led_controller.cpp` | sysfs LED brightness | `led.ring.rgb` |
| Сегментные часы Mini 2 | `yandexmini_2/.../yandex_mini2_clock_led_controller.cpp` | все 32 `mini2:white:clock*/brightness` | `display.clock.segment.v1` |
| Освещённость | `libs/brightness_control/light_sensor.cpp` | `iio:device*/in_intensity_input` | `sensor.ambient_light.iio` |
| Температура | portable PowerManager и стандартный Linux hwmon | thermal-zone/hwmon sysfs | `sensor.temperature.sysfs` |
| Батарея и заряд | `jbl_link/portable/power_manager/power_manager.cpp` | power_supply capacity/status sysfs | `battery.sysfs` |
| Подсветка экрана | Android/TV platform конфигурации | `/sys/class/backlight` | `display.backlight` |
| Кнопки | platform-specific ButtonsHandler | `/sys/class/input` и `/proc/bus/input/devices` | `input.evdev` |

## Не предоставляется намеренно

- OTA, factory reset, ADB activation, power-off и прошивка Bluetooth/Zigbee контроллеров;
- platform keys, device cryptography и идентификаторы устройства;
- внутренние IPC SDK: голосовой pipeline, авторизация, облачные директивы и состояние медиа;
- Zigbee NCP: SDK использует специфичные UART-протоколы и persistent network state, которые нельзя безопасно открывать произвольному пакету без отдельного адаптера и явной модели разрешений.

Это ограничение предотвращает блокировку колонки, утечку ключей и повреждение пользовательской конфигурации. При появлении стабильного локального интерфейса для конкретной возможности его следует добавлять как новый capability, профиль оборудования и минимальный permission, а не как raw sysfs/exec доступ.
