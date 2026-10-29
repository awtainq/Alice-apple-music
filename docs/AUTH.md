# Личный профиль

Для первой установки нужен собственный авторизованный профиль с действующей
подпиской Apple Music. Пакет не содержит аккаунтов, токенов или личных ключей.
При обновлении `.yap` профиль сохраняется.

На компьютере подготовьте папку `my-profile/`:

```text
my-profile/
├── config.json
├── client_id.bin
└── private_key.pem
```

Скопируйте [config.example.json](../config.example.json) в `config.json` и
заполните собственными данными:

| Поле | Содержание |
| --- | --- |
| `apple_app_token` | Собственный авторизованный app/developer token Apple Music-клиента |
| `apple_user_token` | Пользовательский токен владельца подписки |
| `yandex_music_token` | Токен владельца Станции для Glagol |
| `yandex_x_token` | Токен обновления Яндекса, дающий доступ шире одной колонки |
| `station_device_id` | Серийный идентификатор своей Станции |
| `station_platform` | Для проверенной Станции Миди — `yandexmidi` |
| `agent_token` | Случайная строка для локального API, например результат `openssl rand -hex 32` |
| `client_id.bin`, `private_key.pem` | Собственные разрешённые данные уже авторизованного клиента воспроизведения |

Первичный вход в аккаунты выполняется отдельно. Мод не получает ресурсы чужих
аккаунтов и не извлекает их из сторонних хранилищ.

## Передача на колонку

До включения сервиса в Luna передайте три файла по SSH. Выполняйте из папки,
содержащей `my-profile`; замените IP и порт на свои:

```sh
for file in config.json client_id.bin private_key.pem; do
  ssh -p 2222 root@IP_КОЛОНКИ \
    "umask 077; mkdir -p /data/opt/luna/data/apple-music-private; chmod 700 /data/opt/luna/data/apple-music-private; cat > /data/opt/luna/data/apple-music-private/$file; chmod 600 /data/opt/luna/data/apple-music-private/$file" \
    < "my-profile/$file" || break
done
```

Путь рассчитан на Luna с data-dir `/data/opt/luna/data`. При другом data-dir
используйте соседний с `plugins/` каталог `apple-music-private`.

Не загружайте профиль в GitHub, issues или чат. Файлы с токенами и закрытыми
данными должны иметь права 600, каталог — 700. При истечении токена остановите
сервис, обновите соответствующее поле `config.json` и включите сервис снова.
