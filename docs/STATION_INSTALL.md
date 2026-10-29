# Установка на Станцию

Плеер устанавливается как локальный `.yap` для Luna.
Пошаговая инструкция: [INSTALLER.md](INSTALLER.md).

Проверено: рутованная Станция 2 (`yandex,station-midi`, Luna `yandexmidi`),
ARM64, Entware FFmpeg 6.1.4. Профиль:
`/data/opt/luna/data/apple-music-private`, кэш:
`/tmp/alice-apple-music-cache`, API только `127.0.0.1:8099`.

В Luna пакет `alice-apple-music` запускается как binary service с правом exec.
Автозапуск включает **service/enable**. После рестарта очередь восстанавливается
на паузе. Полная перезагрузка колонки пока не проверялась.
