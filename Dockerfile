# syntax=docker/dockerfile:1.7
FROM rust:bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake ninja-build clang libclang-dev perl golang \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY src ./src
COPY web ./web
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked \
    && cp target/release/gemini-rust-gateway /tmp/gemini-rust-gateway

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home gateway \
    && mkdir -p /data \
    && chown gateway:gateway /data

COPY --from=builder /tmp/gemini-rust-gateway /usr/local/bin/gemini-rust-gateway
ENV GEMINI_HOST=0.0.0.0 \
    GEMINI_PORT=8000 \
    GEMINI_DATA_DIR=/data \
    GEMINI_ACCOUNTS_FILE=/data/accounts.local.json \
    GEMINI_PUBLIC_BASE_URL=http://127.0.0.1:8000
VOLUME ["/data"]
EXPOSE 8000
USER gateway
ENTRYPOINT ["gemini-rust-gateway"]
CMD ["serve"]
