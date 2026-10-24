# Реестр пакетов

Публичный реестр расположен в [`nyakokitsu/yapkg`](https://github.com/nyakokitsu/yapkg). Это статический GitHub-реестр: `index.json` находится в ветке `registry`, а `.yap`-архивы — только в ветке `packages`.

```sh
yam registry list
yam install aurora
yam install aurora 1.0.0
```

`yam install <путь-к-yap>` устанавливает локальный файл. `yam install <id>` скачивает последнюю доступную версию из реестра, а `yam install <id> <version>` — конкретную. Эквивалентны команды `yam registry pull <id>` и `yam registry pull <id> <version>`.

По умолчанию CLI использует опубликованный GitHub raw index. Альтернативный статический index можно передать через `--registry URL` или `YAM_REGISTRY_URL`.

Новый пакет добавляется pull request'ом в ветку `packages` репозитория `yapkg`, по пути `packages/<id>/<version>.yap`. CI проверяет архив и manifest; после merge CI обновляет `index.json` в ветке `registry`.
