FROM lukemathwalker/cargo-chef:latest-rust-alpine AS chef
RUN apk add --no-cache protoc protobuf-dev
RUN apk add --no-cache nasm
RUN apk add --no-cache cmake
RUN apk add --no-cache musl-dev
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Собираем с полностью статической линковкой
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

# Теперь можем использовать scratch, так как бинарник полностью статический
FROM scratch

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/rrtc /rrtc

EXPOSE 8080
EXPOSE 5000/udp

ENTRYPOINT ["/rrtc"]