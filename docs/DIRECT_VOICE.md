# Голосовые команды

Плеер читает локальные события распознавания через IPC `127.0.0.1:9875`.
Собственные музыкальные запросы отменяют нативную интерпретацию Алисы и
передаются очереди Apple Music. Прочие функции Алисы остаются у неё.

Короткие слова без обращения («дальше», «назад», «пауза», «играй») приходят
также из command spotter в `/data/quasar/daemons_logs/maind.log`. Обработчик
следит за ротацией журнала и подавляет дубли между двумя источниками.

«Алиса, что играет?» озвучивает название и исполнителя через локальный RPC.
После озвучки плеер восстанавливает свой поток, если Алиса его приостановила.
Успешные включения и переключения не сопровождаются отдельной фразой.

При активном AirPlay локальный обработчик уступает команды Luna и телефону.

## Источники протокола

Механизм канала описан в **Yandex Station Extended — Марк Писаренко**:
[рецепт Spotify](https://hackalice.me/recipes/spotify/),
[исходный alice-text.c](https://github.com/Marker284/yandex-station-extended/blob/main/contrib/station2-media/build/voice/alice-text.c),
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
Транспорт реализован независимо на Rust.

Формат vocalize RPC также документирован в rison1337/Alisa-ChatGPT-voice-assistant,
YSE contrib/alice-say.py и [v3rt3p/quasar-protos](https://github.com/v3rt3p/quasar-protos).
