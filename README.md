# YAM

yam = Y Application Manager. Ставьте на свою колонку .yap.

Linux / macOS:
```sh
curl -fsSL https://raw.githubusercontent.com/nyakokitsu/yam/main/install.sh | sh
```

Windows:

```powershell
irm https://raw.githubusercontent.com/nyakokitsu/yam/main/install.ps1 | iex
```

Windows CLI управляет колонкой: `yam --url http://<speaker-ip>:8099 list`.

# Документация

- [Разработка](docs/DEVELOPMENT.md)
- [API Плагинов](docs/PLUGIN_API.md)
- [Деплой](docs/DEPLOYMENT.md)
- [Пакеты](docs/PACKAGES.md)
- [Реестр пакетов](docs/REGISTRY.md)
- [Аппаратные возможности](docs/PLATFORM_HARDWARE.md)
- [События IPC](docs/IPC.md)
- [Для нейронок](AGENTS.md)

Примеры [`examples/`](examples/).
