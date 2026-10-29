# Установка

Это руководство для отдельного личного развёртывания. Примеры адресов нужно заменить своими. Текущая архитектура использует Linux-плеер, Google Cloud Run, Firestore и Pub/Sub.

## 1. Авторизация и конфигурация

Выполните начальные команды из README, подготовьте файлы из [AUTH.md](AUTH.md), затем создайте закрытые секреты связи:

```sh
.venv/bin/python scripts/generate_secrets.py --private private
```

Скрипт обновит `private/config.json` и сохранит три отдельных файла для Secret Manager. Не меняйте секреты отдельно только на одной стороне.

## 2. Google Cloud

Установите Google Cloud CLI, войдите в свой аккаунт, создайте проект с включённым биллингом. В консоли Яндекс Диалогов создайте черновик приватного навыка и получите его Skill ID. Иконка лежит в `icon.png`.

Следующие команды выполняются из корня репозитория. Для первоначальной настройки аккаунту нужны права создания ресурсов и назначения перечисленных IAM-ролей:

```sh
export PROJECT_ID=YOUR_PROJECT_ID
export SKILL_ID=YOUR_YANDEX_SKILL_ID
export REGION=europe-west1
export DATABASE=alice-music

gcloud services enable run.googleapis.com cloudbuild.googleapis.com \
  artifactregistry.googleapis.com pubsub.googleapis.com firestore.googleapis.com \
  secretmanager.googleapis.com --project "$PROJECT_ID"

gcloud firestore databases create --database "$DATABASE" --location "$REGION" \
  --type firestore-native --project "$PROJECT_ID"
gcloud pubsub topics create alice-music-home --project "$PROJECT_ID"
gcloud pubsub subscriptions create alice-music-home --topic alice-music-home \
  --enable-message-ordering --ack-deadline 60 --message-retention-duration 1h \
  --expiration-period never --project "$PROJECT_ID"

gcloud iam service-accounts create alice-music-webhook --project "$PROJECT_ID"
gcloud iam service-accounts create alice-music-player --project "$PROJECT_ID"

gcloud pubsub topics add-iam-policy-binding alice-music-home \
  --member "serviceAccount:alice-music-webhook@$PROJECT_ID.iam.gserviceaccount.com" \
  --role roles/pubsub.publisher --project "$PROJECT_ID"
gcloud projects add-iam-policy-binding "$PROJECT_ID" \
  --member "serviceAccount:alice-music-webhook@$PROJECT_ID.iam.gserviceaccount.com" \
  --role roles/datastore.user

gcloud pubsub subscriptions add-iam-policy-binding alice-music-home \
  --member "serviceAccount:alice-music-player@$PROJECT_ID.iam.gserviceaccount.com" \
  --role roles/pubsub.subscriber --project "$PROJECT_ID"
gcloud iam service-accounts keys create private/pubsub-home.json \
  --iam-account "alice-music-player@$PROJECT_ID.iam.gserviceaccount.com" --project "$PROJECT_ID"
chmod 600 private/pubsub-home.json

gcloud secrets create alice-music-hook --data-file private/webhook-token.txt --project "$PROJECT_ID"
gcloud secrets create alice-music-agent --data-file private/agent-token.txt --project "$PROJECT_ID"
gcloud secrets create alice-music-bind --data-file private/bind-code.txt --project "$PROJECT_ID"
for SECRET_NAME in alice-music-hook alice-music-agent alice-music-bind; do
  gcloud secrets add-iam-policy-binding "$SECRET_NAME" --project "$PROJECT_ID" \
    --member "serviceAccount:alice-music-webhook@$PROJECT_ID.iam.gserviceaccount.com" \
    --role roles/secretmanager.secretAccessor
done

bash scripts/deploy-cloud.sh
```

Команды создания рассчитаны на новый проект: существующие ресурсы повторно создавать не нужно. Для развёртывания из исходников Google Cloud также потребует настроенную сборку Cloud Build и соответствующие права её сервисного аккаунта; выполните подсказки CLI для своего проекта.

Скопируйте URL созданного Cloud Run в `relay_url`. Заполните в `private/config.json` значения `project`, `skill_id`, `subscription` (`alice-music-home`) и `activation_name`.

Для автоматической очистки временных результатов:

```sh
gcloud firestore fields ttls update expires_at --collection-group music_jobs \
  --database "$DATABASE" --enable-ttl --project "$PROJECT_ID"
```

## 3. Яндекс Диалоги

В черновике навыка задайте:

- Имя и активационное имя: «моя фонотека».
- Приватный доступ, поверхность Яндекс Станция без требования экрана.
- Webhook: `https://YOUR_CLOUD_RUN_URL/alice/YOUR_WEBHOOK_TOKEN`.
- Короткое описание функций и свою информацию разработчика.

Сохраните и опубликуйте навык. После запуска плеера откройте навык на своей Станции и передайте команду `привязка` с вашим 24-значным `bind_code`. Код связывает поверхность и пользователя с этой установкой. Его не нужно оставлять в публичных материалах.

Привязка только ограничивает управление плеером. Она не заменяет разрешение владельца на доступ к аккаунту Яндекса, уже необходимое для Glagol API.

## 4. Сеть: дома или через VPN

Сервер должен устанавливать соединение с IP Станции на порту 1961. Станция должна получать HTTP-аудио по адресу `bind_host:port` из вашей конфигурации.

Для VPS настройте VPN в домашнюю сеть. Примеры в `deploy/` используют:

```text
роутер:      10.10.10.1
VPS:         10.10.10.5
домашняя LAN: 192.168.1.0/24
аудио:       10.10.10.5:8099
```

В существующий AmneziaWG на роутере добавьте отдельный peer VPS с собственными ключами и AllowedIPs `10.10.10.5/32`. На VPS peer роутера должен разрешать `10.10.10.1/32` и домашнюю LAN. В роутере разрешите соответствующее перенаправление между VPN и LAN. Убедитесь, что у Станции есть обратный маршрут через её шлюз.

`deploy/music-home.conf.example` содержит заглушки. Параметры AmneziaWG должны соответствовать вашему роутеру. Установите официальный `amneziawg-go` и инструменты `awg`; версии исходников проверенной установки закреплены в `deploy/vpn-sources.json`. Образец службы предполагает их размещение в `/usr/local/bin`.

```sh
sudo install -d -m 700 /etc/amnezia/amneziawg
# Подготовьте закрытый /etc/amnezia/amneziawg/music-home.conf.
# Для awg setconf не включайте поля Address и MTU из образца:
# адрес, MTU и маршруты задаёт alice-music-vpn-up.
sudo install -d -m 755 /usr/local/libexec
sudo install -m 755 deploy/alice-music-vpn-up /usr/local/libexec/alice-music-vpn-up
sudo install -m 644 deploy/alice-music-vpn.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now alice-music-vpn.service
```

Если пути каталогов отсутствуют, создайте их перед установкой файлов. Не заменяйте рабочие VPN-конфигурации целиком: используйте отдельный интерфейс `music-home`. Аудиосервер на VPS привязывайте к IP VPN, не к публичному интерфейсу.

## 5. Нативный плеер на Debian

Из корня подготовленного репозитория:

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends python3-venv ffmpeg
sudo useradd --system --home-dir /var/lib/alice-music --shell /usr/sbin/nologin alice-music
sudo install -d -m 755 /opt/alice-music/app
sudo cp apple.py engine.py station_client.py service.py requirements.txt /opt/alice-music/app/
sudo python3 -m venv /opt/alice-music/venv
sudo /opt/alice-music/venv/bin/pip install -r /opt/alice-music/app/requirements.txt
sudo install -d -o alice-music -g alice-music -m 700 /var/lib/alice-music/private /var/cache/alice-music
sudo cp -a private/. /var/lib/alice-music/private/
sudo chown -R alice-music:alice-music /var/lib/alice-music/private
sudo chmod 700 /var/lib/alice-music/private
sudo chmod 600 /var/lib/alice-music/private/*
sudo install -m 644 deploy/alice-music.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now alice-music.service
```

Если пользователь уже существует, пропустите `useradd`. Для установки в домашней LAN без VPN удалите из образца плеера зависимость `Requires=alice-music-vpn.service` и соответствующее имя из `After=`. Адрес `station_host` должен принадлежать вашей Станции, `bind_host` — интерфейсу сервера, доступному колонке. В Linux укажите `ffmpeg: /usr/bin/ffmpeg`.

## Проверка и управление

```sh
systemctl status alice-music.service alice-music-vpn.service
journalctl -u alice-music.service -n 30 --no-pager
sudo /opt/alice-music/venv/bin/python scripts/control.py --private /var/lib/alice-music/private
sudo systemctl restart alice-music.service
```

Последняя команда управления предполагает запуск из корня клона репозитория. Воспроизведение проверяйте с выключенным резервным экземпляром плеера. После перезапуска очередь восстанавливается на паузе; скажите навыку «продолжай».
