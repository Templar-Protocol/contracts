# Templar Liquidator Bot - Multi-stage Docker Build

# ============================================
# Build Stage
# ============================================
FROM rust:1.85-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY client ./client
COPY common ./common
COPY contract ./contract
COPY fuzz ./fuzz
COPY mock ./mock
COPY service ./service
COPY test-utils ./test-utils
COPY tools ./tools
COPY universal-account ./universal-account

# Build the liquidator binary in release mode
RUN cargo build --release -p templar-liquidator --bin liquidator

# Strip debug symbols to reduce binary size
RUN strip target/release/liquidator

# ============================================
# Runtime Stage
# ============================================
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -u 1000 -s /bin/bash liquidator

# Create app directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/liquidator /app/liquidator

# Copy configuration templates
COPY --chown=liquidator:liquidator service/liquidator/scripts ./scripts
COPY --chown=liquidator:liquidator service/liquidator/.env.example ./.env.example

# Set ownership
RUN chown -R liquidator:liquidator /app

# Switch to non-root user
USER liquidator

# Set environment variables
ENV RUST_LOG=info,templar_liquidator=debug
ENV RUST_BACKTRACE=1

# Health check
HEALTHCHECK --interval=60s --timeout=10s --start-period=10s --retries=3 \
    CMD pgrep -x liquidator || exit 1

# Labels for metadata
LABEL org.opencontainers.image.title="Templar Liquidator Bot"
LABEL org.opencontainers.image.description="Inventory-based liquidator bot for Templar Protocol"
LABEL org.opencontainers.image.vendor="Templar Protocol"
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.opencontainers.image.source="https://github.com/templar-protocol/contracts"

# Default command
ENTRYPOINT ["/app/liquidator"]
