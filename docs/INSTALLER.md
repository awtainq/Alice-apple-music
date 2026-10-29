# Установка пакета .yap

Нужны рутованная Станция Миди, ARM64 Entware с `/opt/bin/ffmpeg` и Luna,
поддерживающая локальные binary-service `.yap`. Разрешите
`allow_plugin_binaries = true` в настройках Luna.

## Через интерфейс Luna

1. Скачайте `.yap` и его `.sha256` из [Releases](https://github.com/awtainq/Alice-apple-music/releases/latest).
2. Откройте в Luna раздел **«Установка .yap»** и выберите пакет.
3. Загрузите его кнопкой **POST /api/plugins**.
4. Для первой установки подготовьте [личный профиль](AUTH.md).
5. Включите сервис `alice-apple-music` через **service/enable**. Это включает
   автозапуск и запускает плеер.

Пакет содержит бинарный файл ARM64, манифест Luna, инструкцию и лицензии.
Профиль и очередь хранятся вне пакета и сохраняются при обновлениях.

## Через локальный API Luna

Если веб-интерфейс отключён, откройте на компьютере SSH-туннель и оставьте его
работать. Замените IP и SSH-порт на адрес своей колонки. В примере API Luna
слушает `127.0.0.1:8098`:

```sh
ssh -p 2222 -N -L 18098:127.0.0.1:8098 root@IP_КОЛОНКИ
```

В другом терминале компьютера загрузите скачанный пакет:

```sh
curl --fail-with-body -F 'package=@alice-apple-music-luna-0.2.2-aarch64.yap' \
  http://127.0.0.1:18098/api/plugins
```

После добавления профиля включите сервис:

```sh
curl --fail-with-body -X POST -H 'Content-Type: application/json' -d '{}' \
  http://127.0.0.1:18098/api/plugins/alice-apple-music/service/enable
```

Локальный порт туннеля доступен только на компьютере, где запущен SSH.
Если ваша Luna использует другой порт, измените удалённую часть туннеля.

## Обновление

Остановите сервис кнопкой **stop** или запросом:

```sh
curl --fail-with-body -X POST -H 'Content-Type: application/json' -d '{}' \
  http://127.0.0.1:18098/api/plugins/alice-apple-music/stop
```

Загрузите новый `.yap`, затем выполните **service/enable**. Профиль остаётся в
`/data/opt/luna/data/apple-music-private`; очередь восстанавливается на паузе.
Не удаляйте этот каталог при переустановке.

## Отключение

Для отмены автозапуска используйте **service/disable**:

```sh
curl --fail-with-body -X POST -H 'Content-Type: application/json' -d '{}' \
  http://127.0.0.1:18098/api/plugins/alice-apple-music/service/disable
```

Удаление пакета через Luna не удаляет личный профиль из отдельного каталога.
Для отката установите сохранённый предыдущий `.yap` тем же способом.
