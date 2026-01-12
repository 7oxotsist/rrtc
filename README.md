# Rust WebRTC SFU Server (rrtc)

Полнофункциональный Selective Forwarding Unit (SFU) сервер на Rust для WebRTC конференций с поддержкой TURN/STUN, screen sharing и multi-track маршрутизации.

## 🚀 Возможности

- ✅ **SFU архитектура** - Эффективная маршрутизация медиа-потоков между участниками
- ✅ **TURN/STUN поддержка** - Работа через NAT с настраиваемыми ICE серверами
- ✅ **Screen Sharing** - Отдельная маршрутизация для демонстрации экрана
- ✅ **Multi-track** - Поддержка нескольких треков (камера, экран, аудио)
- ✅ **WebSocket Signaling** - Надежный signaling протокол
- ✅ **Масштабируемость** - Поддержка множества комнат и участников
- ✅ **Гибкая конфигурация** - TOML/JSON файлы или переменные окружения
- ✅ **Автоочистка** - Автоматическое удаление пустых комнат

## 📋 Требования

- Rust 1.70+ (edition 2024)
- Tokio runtime
- TURN/STUN сервер (например, coturn)
- WebSocket клиент (фронтенд)

## 🔧 Установка

### 1. Сборка проекта

```bash
cd KursAch/rrtc
cargo build --release
```

### 2. Конфигурация

#### Вариант А: Использование файла конфигурации

Создайте `config.toml` на основе примера:

```bash
cp config.example.toml config.toml
```

Отредактируйте `config.toml` под ваши нужды:

```toml
signaling_port = 8080
listen_address = "0.0.0.0"
max_participants_per_room = 50

[[ice_servers]]
urls = ["stun:stun.l.google.com:19302"]

[[ice_servers]]
urls = ["turn:your-turn-server.com:3478"]
username = "your-username"
credential = "your-password"
```

#### Вариант Б: Переменные окружения

Создайте `.env` файл:

```bash
cp .env.example .env
```

Настройте переменные:

```env
SIGNALING_PORT=8080
TURN_URL=turn:your-turn-server.com:3478
TURN_USERNAME=your-username
TURN_CREDENTIAL=your-password
```

### 3. Настройка TURN сервера (coturn)

#### Установка coturn

```bash
# Ubuntu/Debian
sudo apt-get install coturn

# или через Docker (см. docker-compose.yaml в корне проекта)
```

#### Минимальная конфигурация coturn

Создайте `/etc/turnserver.conf`:

```conf
listening-port=3478
listening-ip=0.0.0.0
relay-ip=YOUR_SERVER_PUBLIC_IP
external-ip=YOUR_SERVER_PUBLIC_IP

fingerprint
lt-cred-mech
user=webrtc:your-secure-password
realm=your-domain.com

no-tlsv1
no-tlsv1_1
```

Запустите coturn:

```bash
sudo systemctl enable coturn
sudo systemctl start coturn
```

## 🚀 Запуск

### Режим разработки

```bash
# С логированием debug уровня
RUST_LOG=debug cargo run

# С конфигурационным файлом
cargo run -- config.toml
```

### Продакшн режим

```bash
# Собрать release версию
cargo build --release

# Запустить
./target/release/rrtc

# Или с конфигурацией
./target/release/rrtc config.toml
```

### Через Docker

```bash
# Сборка образа
docker build -t rrtc-sfu .

# Запуск
docker run -d \
  -p 8080:8080 \
  -e TURN_URL=turn:your-server.com:3478 \
  -e TURN_USERNAME=webrtc \
  -e TURN_CREDENTIAL=password \
  rrtc-sfu
```

### Через docker-compose

```bash
# Из корня проекта
cd ..
docker-compose up -d rrtc coturn
```

## 📡 Signaling Protocol

### Сообщения от клиента к серверу

#### Join - Присоединение к комнате
```json
{
  "type": "join",
  "room": "room-id",
  "participant": "user-id",
  "name": "User Name"
}
```

#### Offer - WebRTC Offer
```json
{
  "type": "offer",
  "sdp": "v=0\r\n..."
}
```

#### ICE Candidate
```json
{
  "type": "candidate",
  "candidate": "candidate:..."
}
```

#### State Update - Обновление состояния
```json
{
  "type": "state_update",
  "muted": false,
  "video_on": true,
  "screen_sharing": false
}
```

#### Screen Sharing Control
```json
{
  "type": "start_screen_share"
}

{
  "type": "stop_screen_share"
}
```

### Сообщения от сервера к клиенту

#### Joined - Подтверждение присоединения
```json
{
  "type": "joined",
  "your_id": "user-id",
  "participants": [
    {
      "id": "other-user-id",
      "name": "Other User",
      "muted": false,
      "video_on": true,
      "screen_sharing": false
    }
  ]
}
```

#### Answer - WebRTC Answer
```json
{
  "type": "answer",
  "sdp": "v=0\r\n..."
}
```

#### ICE Candidate
```json
{
  "type": "candidate",
  "candidate": "candidate:..."
}
```

#### Participant Joined/Left
```json
{
  "type": "participant_joined",
  "id": "user-id",
  "name": "User Name"
}

{
  "type": "participant_left",
  "participant_id": "user-id"
}
```

#### State Update
```json
{
  "type": "state_update",
  "participant_id": "user-id",
  "muted": true,
  "video_on": false,
  "screen_sharing": false
}
```

#### Error
```json
{
  "type": "error",
  "message": "Error description",
  "code": 403
}
```

## 🏗️ Архитектура

### Модули

- **main.rs** - Точка входа, WebSocket сервер, обработка соединений
- **peer.rs** - Управление WebRTC peer connections
- **room.rs** - Управление комнатами и маршрутизация медиа
- **messages.rs** - Определение протокола signaling
- **config.rs** - Конфигурация и ICE серверы

### Поток данных

```
Client A          SFU Server         Client B
   |                  |                  |
   |--Join----------->|                  |
   |<-Joined----------|                  |
   |                  |<-Join------------|
   |<-ParticipantJoined                  |
   |                  |-Joined---------->|
   |--Offer---------->|                  |
   |<-Answer----------|                  |
   |--ICE Candidate-->|                  |
   |                  |--Offer---------->|
   |                  |<-ICE Candidate---|
   |                  |                  |
   |==RTP Packets====>|==RTP Packets===>|
   |                  |                  |
```

### Track Routing

1. Клиент отправляет offer с треками (audio, video, screen)
2. SFU создает answer и настраивает обработчики
3. При получении RTP пакетов, SFU определяет тип трека
4. Пакеты маршрутизируются всем другим участникам комнаты
5. Фильтрация на основе состояния (muted, video_on, screen_sharing)

### Типы треков

- **TrackType::Audio** - Аудио поток (микрофон)
- **TrackType::Camera** - Видео с камеры
- **TrackType::Screen** - Screen sharing

Тип определяется по track.id():
- Содержит "screen" → Screen
- Содержит "audio" → Audio
- Иначе → Camera

## 🔒 Безопасность

### Рекомендации для продакшена

1. **Используйте HTTPS/WSS** - Настройте nginx reverse proxy с TLS
2. **Защитите TURN сервер** - Используйте сложные credentials
3. **Ограничьте доступ** - Firewall правила для портов
4. **Регулярные обновления** - Обновляйте зависимости
5. **Мониторинг** - Логируйте и отслеживайте аномалии

### Пример nginx конфигурации

```nginx
upstream rrtc_backend {
    server localhost:8080;
}

server {
    listen 443 ssl http2;
    server_name your-domain.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location /ws {
        proxy_pass http://rrtc_backend;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 86400;
    }
}
```

## 📊 Мониторинг и отладка

### Логирование

Управление уровнем логов через `RUST_LOG`:

```bash
# Базовое логирование
RUST_LOG=info cargo run

# Детальное логирование
RUST_LOG=debug cargo run

# Только WebRTC и rrtc
RUST_LOG=webrtc=debug,rrtc=debug cargo run

# Трассировка всего
RUST_LOG=trace cargo run
```

### Отладка ICE соединений

1. Проверьте доступность STUN/TURN серверов:
```bash
# Проверка STUN
stunclient stun.l.google.com 19302

# Проверка TURN (требует turnutils из coturn)
turnutils_uclient -v -u username -w password your-turn-server.com
```

2. Проверьте открытые порты:
```bash
# WebSocket signaling
netstat -tulpn | grep 8080

# TURN/STUN
netstat -tulpn | grep 3478
```

3. Логи клиента - включите в браузере:
```javascript
// В консоли браузера
localStorage.setItem('debug', 'webrtc:*');
```

## 🧪 Тестирование

### Локальное тестирование

```bash
# Запустите сервер
cargo run

# В другом терминале - подключите тестовый клиент
# (требуется фронтенд из ../frontend)
```

### Тестирование в разных сетях

1. Разверните сервер на публичном хосте
2. Настройте TURN с правильным external-ip
3. Подключитесь с устройств в разных сетях
4. Проверьте ICE candidates в логах (должны быть relay типа)

### Unit тесты

```bash
# Запуск тестов
cargo test

# С подробным выводом
cargo test -- --nocapture

# Только определенный модуль
cargo test config::tests
```

## 🐛 Известные проблемы и решения

### Проблема: Соединение не устанавливается через NAT

**Решение:** 
- Убедитесь, что TURN сервер настроен правильно
- Проверьте `external-ip` в конфигурации coturn
- Откройте UDP порты 3478 и relay range (49152-65535)

### Проблема: Видео не отображается у других участников

**Решение:**
- Проверьте, что offer содержит треки (в логах)
- Убедитесь, что track.id() правильно парсится
- Проверьте состояние video_on у отправителя

### Проблема: High CPU usage

**Решение:**
- Уменьшите количество участников в комнате
- Используйте release сборку (`cargo build --release`)
- Рассмотрите аппаратное кодирование на клиентах

## 📚 Документация WebRTC API

- [webrtc-rs docs](https://docs.rs/webrtc/latest/webrtc/)
- [WebRTC для начинающих](https://webrtc.org/getting-started/overview)
- [MDN WebRTC API](https://developer.mozilla.org/en-US/docs/Web/API/WebRTC_API)

## 🤝 Интеграция с фронтендом

Фронтенд находится в `../frontend`. Основные шаги интеграции:

1. Подключитесь к WebSocket: `ws://localhost:8080` (или `wss://` для продакшена)
2. Отправьте сообщение `join`
3. Создайте RTCPeerConnection с ICE серверами из конфигурации
4. Отправьте offer после добавления треков
5. Обработайте answer и ICE candidates от сервера
6. Маршрутизация медиа происходит автоматически

Пример клиентского кода в `../frontend/src/services/webrtc.service.ts`

## 🔄 Обновление и миграция

### От str0m к webrtc-rs

Этот сервер является полной переработкой с использованием `webrtc-rs` вместо `str0m`. Основные изменения:

- ✅ Полная поддержка TURN/STUN
- ✅ Модульная архитектура
- ✅ Улучшенная обработка треков
- ✅ Поддержка screen sharing на уровне типов
- ✅ Конфигурация через файлы и ENV

## 📝 Лицензия

MIT

## 👥 Авторы

KursAch WebRTC SFU Team

## 🆘 Поддержка

При возникновении проблем:
1. Проверьте логи сервера (`RUST_LOG=debug`)
2. Проверьте логи браузера (F12 → Console)
3. Убедитесь в правильности конфигурации TURN/STUN
4. Проверьте сетевые подключения (firewall, NAT)

---

**Статус:** Production Ready ✅
**Версия:** 0.1.0
**Последнее обновление:** 2024