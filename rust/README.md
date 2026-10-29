# Alice × Apple Music · Luna

Нативный плеер Apple Music для рутованной Станции Миди: один процесс Rust,
голосовой поиск, очередь, рекомендации и название трека.

## Установка

Скачайте `alice-apple-music-luna-0.2.2-aarch64.yap` из
[Releases](https://github.com/awtainq/Alice-apple-music/releases/latest) и загрузите
его в Luna через **«Установка .yap»**. Добавьте личный профиль и включите сервис
**Alice × Apple Music** (`alice-apple-music`).

Нужны ARM64 Entware с `/opt/bin/ffmpeg`, совместимая Luna с поддержкой локальных
binary services и разрешением `allow_plugin_binaries = true`.

[Инструкция установки](https://github.com/awtainq/Alice-apple-music/blob/main/docs/INSTALLER.md) ·
[Авторизация](https://github.com/awtainq/Alice-apple-music/blob/main/docs/AUTH.md)

Пакет использует root/exec и напрямую читает локальный IPC и command spotter
для произвольных названий песен. Профиль и очередь находятся вне пакета,
в соседнем с `plugins/` каталоге `apple-music-private`. Обновление `.yap` их
не удаляет. Кэш находится в `/tmp/alice-apple-music-cache`.

## Локальный API

Только `127.0.0.1:8099`. GET `/api` возвращает состояние,
POST `/api` принимает команды: `{"action":"next"}`. Оба запроса требуют
`Authorization: Bearer <agent_token>`. Аудио защищено случайным HMAC-токеном
каждого запуска и поддерживает HTTP Range.

При подключении AirPlay очередь ставится на паузу и уступает команды телефону.
Для этого в профиле задайте `airplay_bus` равным `/run/station-airplay/private-bus`.
Отключение телефона не возобновляет локальную очередь автоматически.

## Сборка

Нужны Rust stable и target `aarch64-unknown-linux-musl`. На Linux с настроенным
musl cross-linker:

```sh
cargo test --locked --workspace --manifest-path rust/Cargo.toml
bash rust/build.sh
cargo run --locked --manifest-path rust/Cargo.toml -p xtask -- package
```

На macOS или для кросс-компиляции установите Zig 0.13 и cargo-zigbuild 0.24.0,
затем используйте `bash rust/build.sh --zig`. Скрипт нормализует локальные пути
разработчика в исполняемом файле.

Команда `xtask package` создаёт tar.gz `.yap` и SHA256 в `dist/`. Можно передать
`--binary PATH` и `--output PATH`. Сборщик проверяет ARM64 ELF и отсутствие
личных путей, задаёт одинаковые права и временные метки файлов пакета.

Полный архив соответствующих исходников с закреплёнными зависимостями Cargo:

```sh
cargo run --locked --manifest-path rust/Cargo.toml -p xtask -- source
```

Для сборки из этого архива запускайте команды из его корня. Поставщик исходников
Cargo настроен на включённый `vendor/`; используйте `--offline --locked`.
Сам компилятор и средства кросс-компиляции в архив не включены.

Лицензия связанного бинарного файла — **GPL-3.0-only**; см. LICENSE и NOTICE.

## Пакет реестра YAM

`luna-import/` — отдельная нативная команда `awtainq-apple-music` без внешних
библиотек. Она находит соседний с собой `.yap` в payload YAM, останавливает
предыдущий сервис и загружает пакет только на `127.0.0.1`. Порт задаётся числом.
Запрос запуска отправляется только после успешной загрузки. Личные токены
команда не читает. Удалением импортированного плагина управляет Luna.

`bash rust/build.sh --zig` также собирает эту команду. Затем официальный
инструмент YAM создаёт `.yampkg` из [yam/yam.yaml](../yam/yam.yaml):

```sh
yam-publish pack --source yam --platform yandexmidi \
  --profiles /путь/к/yam/platforms \
  --output dist/awtainq-apple-music-0.2.2-yandexmidi.yampkg
```

В `.yampkg` включается уже собранный `.yap`; сначала выполните `xtask package`.
Для подписи и загрузки нужны собственные namespace, ключ автора и publish-токен.
Храните их вне репозитория. Процедура: [YAM Publishing](https://github.com/nyakokitsu/yam/blob/main/docs/PUBLISHING.md).
