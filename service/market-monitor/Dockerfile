# Build stage
FROM rust:1.86 as builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy entire workspace (needed for workspace resolution)
COPY . .

# Build only the market-monitor binary
RUN cargo build --release -p templar-market-monitor

# Runtime stage
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y \
    ca-certificates \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /build/target/release/market-monitor /usr/local/bin/

ENV TZ=UTC
ENV RUST_LOG=info

CMD ["market-monitor"]
