# syntax=docker/dockerfile:1

FROM rust:1.85.0 AS builder

WORKDIR /usr/src/relayer

COPY . .

ENV SQLX_OFFLINE=true
RUN \
    # cache dependencies, etc.
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/rustup \
    cargo build --package templar-relayer --profile release

# Runtime image
FROM debian:12-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends libssl3 && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/src/relayer/target/release/templar-relayer /usr/local/bin/templar-relayer
COPY /service/relayer/config.yaml .
RUN ls -al
RUN pwd

CMD [ "templar-relayer", "--config", "/app/config.yaml" ]
