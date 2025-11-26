# Templar Funding Bridge - Multi-stage Docker Build

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
COPY common ./common
COPY service ./service
COPY contract ./contract
COPY mock ./mock
COPY test-utils ./test-utils
COPY universal-account ./universal-account

# Build the funding-bridge binary in release mode with all features
# Note: You can customize features via build args
ARG FEATURES="ethereum,solana"
RUN cargo build --release --features "${FEATURES}" -p templar-funding-bridge --bin funding-bridge

# Strip debug symbols to reduce binary size
RUN strip target/release/funding-bridge

# ============================================
# Runtime Stage
# ============================================
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -u 1000 -s /bin/bash funding-bridge

# Create app directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/funding-bridge /app/funding-bridge

# Copy configuration templates
COPY --chown=funding-bridge:funding-bridge service/funding-bridge/.env.example ./.env.example
COPY --chown=funding-bridge:funding-bridge service/funding-bridge/scripts/run_service.sh ./run_service.sh

# Set ownership
RUN chown -R funding-bridge:funding-bridge /app && \
    chmod +x /app/funding-bridge /app/run_service.sh

# Switch to non-root user
USER funding-bridge

# Set environment variables
ENV RUST_LOG=info,templar_funding_bridge=debug
ENV RUST_BACKTRACE=1
ENV PORT=3000

# Expose HTTP port
EXPOSE 3000

# Health check using the /health endpoint
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:${PORT}/health || exit 1

# Labels for metadata
LABEL org.opencontainers.image.title="Templar Funding Bridge"
LABEL org.opencontainers.image.description="Multi-chain treasury management service for NEAR Protocol"
LABEL org.opencontainers.image.vendor="Templar Protocol"
LABEL org.opencontainers.image.licenses="GPL-3.0"
LABEL org.opencontainers.image.source="https://github.com/templar-protocol/contracts"
LABEL org.opencontainers.image.documentation="https://github.com/templar-protocol/contracts/tree/main/service/funding-bridge"

# Default command
ENTRYPOINT ["/app/funding-bridge"]

# Default args (can be overridden)
CMD ["--help"]
