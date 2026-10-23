# Адаптер событий IPC

YAM — IPC-клиент, а не прокси и не man-in-the-middle. Поддерживаются только read-only события:

- `alarmd`: `QuasarMessage.alarm_event.alarm_fired` (поле 215);
- `aliced`: `QuasarMessage.alice_state.recognized_phrase` (поле 3).

Транспорт в SDK — TCP с little-endian 32-bit length prefix и protobuf `QuasarMessage`. Адреса в разных прошивках не стабильны, поэтому задаются оператором daemon, а не архивом пакета:

```ini
# systemd override for yamd
[Service]
Environment=YAM_ALARMD_ADDR=127.0.0.1:9912
Environment=YAM_ALICED_ADDR=127.0.0.1:9913
```

Указанные номера приведены только как пример. Перед настройкой следует сверить назначение портов с конфигурацией конкретной прошивки; YAM не сканирует порты и не делает предположений о модели.

Не поддерживаются отправка команд, перехват аудио, токенов, VINS/directive payload и raw protobuf. Эти данные либо чувствительны, либо способны изменить состояние штатных сервисов.
