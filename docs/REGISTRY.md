# Реестр пакетов YAM

`yam-registry` — отдельный сервис публикации и распространения `.yap`. Метаданные хранятся локально в `index.json`, сами архивы — в S3-совместимом бакете. Реестр принимает как прежние ZIP-пакеты, так и новые `tar.gz`-пакеты.

## Запуск

Создайте каталог данных и `registry.toml`:

```toml
s3_bucket = "yam-packages"
s3_region = "us-east-1"
# Для MinIO, Garage и другого S3-compatible storage:
# s3_endpoint = "https://s3.example.net"
hcaptcha_sitekey = "ваш-public-sitekey"
# В production за HTTPS:
session_secure = true
```

Секреты не попадают в файл конфигурации:

```sh
export AWS_ACCESS_KEY_ID='…'
export AWS_SECRET_ACCESS_KEY='…'
export HCAPTCHA_SECRET='…'
cargo run --release --features registry --bin yam-registry -- --data-dir ./registry-data
```

Переменные `YAM_REGISTRY_LISTEN` и `YAM_REGISTRY_DATA_DIR` задают адрес и каталог без аргументов командной строки.

Для systemd есть шаблон [`yam-registry.service`](../yam-registry.service). Соберите только реестр и установите бинарник на сервере:

```sh
cargo build --release --features registry --bin yam-registry
sudo useradd --system --home /var/lib/yam-registry --shell /usr/sbin/nologin yam-registry
sudo install -Dm755 target/release/yam-registry /opt/yam-registry/bin/yam-registry
sudo install -Dm644 yam-registry.service /etc/systemd/system/yam-registry.service
sudo install -d -o yam-registry -g yam-registry /var/lib/yam-registry
sudo systemctl enable --now yam-registry
```

`/etc/yam-registry.env` должен иметь права `0600` и содержать `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` и `HCAPTCHA_SECRET`.

## API

- `GET /api/packages` — список опубликованных версий;
- `GET /api/packages/{id}/{version}/download` — скачивание `.yap` через реестр;
- `POST /api/packages` — публикация multipart-полями `package` и, для прямого API-вызова, `h-captcha-response`;
- `POST /api/auth/hcaptcha` с JSON `{"token":"…"}` — создаёт часовую сессию Web UI.

Web UI доступен по `/`. До решения hCaptcha он показывает только страницу проверки. Публикация через Web UI использует эту сессию; прямой вызов API обязан передать новый hCaptcha-токен. Пакеты ограничены 50 MiB, ID и версия пакета являются неизменяемыми: повторная публикация той же пары отклоняется.

## S3-права

Учетной записи реестра достаточно `s3:PutObject` и `s3:GetObject` в префиксе `packages/` указанного бакета. При `s3_endpoint` реестр автоматически использует S3 path-style, совместимый с MinIO. Не публикуйте ключи S3 и `HCAPTCHA_SECRET` в репозитории, образах или `registry.toml`.

## CLI колонки

CLI получает список и устанавливает пакет из реестра:

```sh
yam --registry https://registry.example.net registry list
yam --registry https://registry.example.net registry pull aurora 1.0.0
```

Адрес можно сохранить в `YAM_REGISTRY_URL`.
