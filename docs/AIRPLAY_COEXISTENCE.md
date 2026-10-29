# Установка Station AirPlay 1.4.14

Установлено из PR #11 Station-airplay с Luna из связанного PR #3. Коммиты и
результаты проверок описаны в истории установки.
AirPlay-устройство: **Yandex Station 2 AirPlay**. Пользователь подтвердил звук
с iPhone и двустороннюю синхронизацию громкости.

## Размещение

- Luna: `/data/opt/luna`, API только `127.0.0.1:8098`.
- Настройки: `/data/opt/luna/data/airplay-controls.json`.
- Пакеты: `/data/opt/luna/data/plugins/station-airplay`, `station-volume`.
- Контрольный приватный D-Bus: `/run/station-airplay/private-bus`.
- Журналы: `/tmp/luna.log`, `/tmp/luna-plugin-state/station-airplay/runtime.log`.
- Автозапуск: `/etc/init.d/S99zyluna`, launcher `/data/opt/luna/launch.sh`.
- Плеер Apple Music остаётся локальным, API `127.0.0.1:8099`.

AirPlay-приёмник не останавливает Quasar или platformd, системная ALSA-конфигурация
не заменялась. При подключении AirPlay собственная очередь Apple Music ставится
на паузу. Её голосовой обработчик уступает управление живому AirPlay-сеансу.
После отключения телефона очередь сама не возобновляется. Чтобы продолжить её,
отключите выход AirPlay на телефоне и скажите «Алиса, продолжай».

## Настройки и границы проверки

Включены voice, ducking, alice_button, native_player и native_volume. Индикация leds
выключена. Физическая кнопка отдельно владельцем не проверялась. Автозапуск настроен,
но полный холодный старт в этой установке не проверялся.

Исходный station-volume не разрешал запись для прошивки
`0.372.1.8.4790676133.20261019.298`. По прямому запросу владельца выполнен
отдельный пробный цикл typed VolumeSet: 30 → 25 → 30. Команда получила
native ACK, отдельное чтение подтвердило оба уровня и восстановление.
После этого в wrapper добавлена **только** пара этой полной версии и
`yandex,station-midi`. Остальные guards сохранены. Тест проверил отказ на
другой плате и непроверенной прошивке, а также прежние разрешённые пары.
Это локальное проверенное дополнение, а не объявление официальной поддержки
любой прошивки 0.372. Оригинал сохранён на колонке:
`/data/opt/luna/station-volume.before-0372.sh`.

## Откат громкости

Сначала установите `native_volume:false` в airplay-controls.json и перезапустите
только приёмник через Luna API. Затем при необходимости верните оригинальный
station-volume.sh из сохранённой копии. Остальные guards не удаляйте.

## Откат AirPlay

Остановите `/data/opt/luna/launch.sh stop`. Для отмены загрузки при старте удалите
только наш `/etc/init.d/S99zyluna` с временным remount `/` в rw и возвратом в ro.
После отключения приёмника локальный голосовой плеер продолжает работать.
Сохраняйте `/data/opt/luna/data/apple-music-private` с личным профилем.

Исходники и бинарники AirPlay/Luna не перепубликовывались. Ссылки на источники:
https://github.com/Yandex-Station-Universal-Repo/Station-airplay/pull/11
https://github.com/nyakokitsu/luna/pull/3
