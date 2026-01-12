# Multi-stage build для оптимальной сборки
FROM rust:1.83-slim as builder

WORKDIR /app

# Установка необходимых системных зависимостей для сборки
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Копируем все файлы проекта
COPY . .

# Собираем релизную версию
RUN cargo build --release

# Финальный минимальный образ
FROM debian:bookworm-slim

# Устанавливаем только runtime зависимости
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    procps \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Копируем собранный бинарник
COPY --from=builder /app/target/release/rrtc /app/rrtc

# Копируем пример конфигурации
COPY --from=builder /app/config.example.toml /app/config.example.toml

# Создаем непривилегированного пользователя
RUN useradd -m -u 1000 rrtc && \
    chown -R rrtc:rrtc /app

USER rrtc

# Порт для WebSocket signaling
EXPOSE 8080

# Переменные окружения по умолчанию
ENV RUST_LOG=info
ENV SIGNALING_PORT=8080
ENV LISTEN_ADDRESS=0.0.0.0

# Healthcheck
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD pgrep rrtc || exit 1

# Запуск приложения
CMD ["/app/rrtc"]
