# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.89
FROM rust:${RUST_VERSION}-bookworm AS builder

WORKDIR /workspace

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target \
    cargo build --locked --release -p templar-hot-relayer && \
    cp target/release/hot-relayer /tmp/hot-relayer

FROM debian:bookworm-slim AS runtime

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /tmp/hot-relayer /usr/local/bin/hot-relayer

USER 10001:10001
WORKDIR /tmp
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ENV RUST_LOG=info,hot_relayer=debug,tower_http=info
EXPOSE 3001
ENTRYPOINT ["/usr/local/bin/hot-relayer"]
