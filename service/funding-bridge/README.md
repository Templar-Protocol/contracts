# Funding Bridge Service

**Multi-Chain Treasury Management for NEAR Protocol**

A NEAR-centric treasury management service with cross-chain deposits and withdrawals via [NEAR Intents Bridge API](https://docs.near-intents.org/). Supports Ethereum, Arbitrum, Base, Optimism, Polygon, and Solana.

[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

## Features

- ✅ **NEAR Treasury** - Hold OMFT tokens on NEAR, withdraw to any chain
- ✅ **Cross-Chain Withdrawals** - NEP-413 signed intents to external chains
- ✅ **Automated Deposits** - Transfer from ETH/Solana wallets to NEAR treasury via bridge
- ✅ **Multi-Chain Support** - Ethereum, Arbitrum, Base, Optimism, Polygon, Solana
- ✅ **Token Resolution** - Automatic OMFT token ID and decimal handling
- ✅ **Stateless Design** - No database required, horizontally scalable
- ✅ **REST API** - Simple HTTP/JSON interface
- ✅ **Status Tracking** - Query status via Bridge API using transaction hash
- ✅ **Prometheus Metrics** - Production-grade observability
- ✅ **Dry-Run Mode** - Test operations without executing transactions

## Quick Start

### Prerequisites

- Rust 1.85 or higher
- NEAR CLI (for key management)
- Access to NEAR RPC endpoint

### Installation

```bash
# Clone the repository
git clone https://github.com/templar-protocol/contracts.git
cd contracts/service/funding-bridge

# Build the service
cargo build --release

# The binary will be at target/release/funding-bridge
```

### Configuration

The service is configured via environment variables or CLI arguments:

```bash
# Required: NEAR treasury configuration
export NEAR_ACCOUNT=treasury.near
export NEAR_SIGNER_KEY="ed25519:YOUR_PRIVATE_KEY_HERE"

# Optional: Service configuration
export PORT=3000
export NETWORK=mainnet
export NEAR_RPC_URL=https://rpc.mainnet.near.org

# Optional: Bridge API (default: https://bridge.chaindefuser.com/rpc)
export BRIDGE_API_URL=https://bridge.chaindefuser.com/rpc

# Run the service
./target/release/funding-bridge
```

Or use CLI arguments:

```bash
./target/release/funding-bridge \
  --port 3000 \
  --network mainnet \
  --near-account treasury.near \
  --near-signer-key "ed25519:YOUR_PRIVATE_KEY_HERE"
```

### Dry-Run Mode

For testing without executing real transactions:

```bash
./target/release/funding-bridge --dry-run \
  --near-account treasury.near \
  --near-signer-key "ed25519:YOUR_PRIVATE_KEY_HERE"
```

## API Documentation

### Base URL

```
http://localhost:3000
```

### Endpoints

#### 1. Health Check

Check service health and view available chains.

```bash
curl http://localhost:3000/health
```

**Response:**
```json
{
  "healthy": true,
  "version": "0.1.0",
  "chains": [
    {
      "name": "near",
      "available": true
    }
  ],
  "bridge_api_status": {
    "reachable": true,
    "latency_ms": 506
  }
}
```

#### 2. Deposit Funds

Transfer tokens from external chain (Ethereum, Solana, etc.) to NEAR treasury via the bridge.

```bash
curl -X POST http://localhost:3000/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "source_chain": "ethereum",
    "asset": "USDC",
    "amount": "100.5",
    "dry_run": false
  }'
```

**Request Fields:**
- `source_chain` - Source chain to transfer from (e.g., "ethereum", "arbitrum", "solana", "eth:42161")
- `asset` - Asset symbol (e.g., "USDC", "USDT")
- `amount` - Amount in human-readable format (e.g., "100.5")
- `dry_run` (optional) - If true, simulate without executing

**Response:**
```json
{
  "source_tx_hash": "0xabc123...",
  "status": "PENDING",
  "source_chain": "eth:1",
  "bridge_deposit_address": "0xdef456..."
}
```

**Status Values:**
- `PENDING` - Transfer submitted, waiting for bridge to credit NEAR treasury
- `SUBMITTED` - Transaction submitted but not yet confirmed
- `DRY_RUN` - Simulated transfer (dry-run mode)
- `FAILED` - Transaction or validation failed

#### 3. Withdraw Funds

Withdraw funds from NEAR to external chain (via bridge). The destination address is configured in the service configuration per chain.

```bash
curl -X POST http://localhost:3000/withdraw \
  -H "Content-Type: application/json" \
  -d '{
    "destination_chain": "ethereum",
    "asset": "USDC",
    "amount": "500000",
    "dry_run": false
  }'
```

**Request Fields:**
- `destination_chain` - Target chain (see supported formats below)
- `asset` - Asset identifier (e.g., "USDC", "USDT", "ETH")
- `amount` - Amount in smallest units
- `dry_run` (optional) - If true, simulate without executing

**Supported Chain Formats:**
```
ethereum / eth / eth:1          → Ethereum Mainnet
arbitrum / arb / eth:42161      → Arbitrum One
base / eth:8453                 → Base
optimism / op / eth:10          → Optimism
polygon / matic / eth:137       → Polygon
solana / sol / sol:mainnet      → Solana Mainnet
```

**Response:**
```json
{
  "source_tx_hash": "ABC123...",
  "status": "COMPLETED",
  "destination_tx_hash": "0x..."
}
```

**Status Values:**
- `COMPLETED` - Withdrawal completed (dry-run mode)
- `PENDING` - Withdrawal initiated, waiting for bridge
- `FAILED` - Transaction or validation failed

The service validates tokens against the NEAR Intents Bridge API and resolves NEAR token IDs automatically (e.g., `eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near` for USDC on Ethereum).

#### 4. Query Status

Get status of a deposit or withdrawal by transaction hash via the Bridge API.

**Withdrawal Status:**
```bash
curl http://localhost:3000/status/withdrawal/ABC123def456
```

**Response:**
```json
{
  "near_tx_hash": "ABC123def456",
  "status": "COMPLETED",
  "chain": "eth:1",
  "destination_tx_hash": "0x...",
  "amount": "1000000"
}
```

**Deposit Status:**
```bash
curl "http://localhost:3000/status/deposit/0xabc123?type=eth"
```

**Response:**
```json
{
  "tx_hash": "0xabc123",
  "status": "COMPLETED",
  "chain": "eth:1",
  "near_tx_hash": "7xyz...",
  "amount": "1000000"
}
```

**Status Values:**
- `COMPLETED` - Operation completed successfully
- `PENDING` - Operation is in progress
- `FAILED` - Operation failed

#### 5. Prometheus Metrics

```bash
curl http://localhost:3000/metrics
```

Returns Prometheus-formatted metrics:
```
# HELP funding_bridge_chain_handler_available Chain handler availability
# TYPE funding_bridge_chain_handler_available gauge
funding_bridge_chain_handler_available{chain="near"} 1

# HELP funding_bridge_deposit_operations_total Total deposit operations
# TYPE funding_bridge_deposit_operations_total counter
funding_bridge_deposit_operations_total{chain="near",status="COMPLETED"} 5

# HELP funding_bridge_withdraw_operations_total Total withdraw operations
# TYPE funding_bridge_withdraw_operations_total counter
funding_bridge_withdraw_operations_total{chain="ethereum",status="COMPLETED"} 3
funding_bridge_withdraw_operations_total{chain="arbitrum",status="COMPLETED"} 2
```

#### 6. Token Lookup

Look up OMFT token ID for an asset on a specific chain.

```bash
curl "http://localhost:3000/tokens/lookup?asset=USDT&chain=ethereum"
```

**Query Parameters:**
- `asset` - Asset name (e.g., "USDT", "USDC", "ETH", "WBTC")
- `chain` - Chain identifier (e.g., "ethereum", "eth:1", "arbitrum", "eth:42161")

**Response:**
```json
{
  "asset": "USDT",
  "chain": "eth:1",
  "omft_token_id": "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near",
  "decimals": 6,
  "bridge_info": {
    "asset_name": "USDT",
    "chain_type": "eth",
    "chain_id": "1",
    "decimals": 6,
    "defuse_asset_identifier": "eth:1:0xdac17f958d2ee523a2206206994597c13d831ec7"
  }
}
```

## Use Cases

### 1. Liquidation Bot Pre-funding

Automatically deposit USDC from Ethereum to top up NEAR treasury:

```bash
curl -X POST http://localhost:3000/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "liq-'$(date +%s)'",
    "source_chain": "ethereum",
    "asset": "USDC",
    "amount": "1000.0"
  }'
```

### 2. Manual Treasury Operations

CLI script for depositing from different chains:

```bash
#!/bin/bash
REQUEST_ID="manual-$(date +%s)"
CHAIN="$1"   # ethereum, arbitrum, solana
AMOUNT="$2"  # e.g., "100.5"

curl -X POST http://localhost:3000/deposit \
  -H "Content-Type: application/json" \
  -d "{
    \"request_id\": \"$REQUEST_ID\",
    \"source_chain\": \"$CHAIN\",
    \"asset\": \"USDC\",
    \"amount\": \"$AMOUNT\"
  }"

# Poll status
sleep 2
curl http://localhost:3000/status/$REQUEST_ID
```

### 3. Programmatic Deposit Integration

```rust
use reqwest::Client;
use serde_json::json;

async fn deposit_from_ethereum(amount: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = Client::new();
    let response = client
        .post("http://localhost:3000/deposit")
        .json(&json!({
            "request_id": format!("auto-{}", chrono::Utc::now().timestamp()),
            "source_chain": "ethereum",
            "asset": "USDC",
            "amount": amount,
        }))
        .send()
        .await?;

    let result: serde_json::Value = response.json().await?;
    Ok(result["source_tx_hash"].as_str().unwrap_or("").to_string())
}
```

## Configuration Reference

### CLI Arguments

```
USAGE:
    funding-bridge [OPTIONS]

OPTIONS:
    --port <PORT>
        HTTP server port [default: 3000]

    --network <NETWORK>
        NEAR network: mainnet or testnet [default: testnet]

    --near-rpc-url <URL>
        Custom NEAR RPC URL (overrides network default)

    --bridge-api-url <URL>
        NEAR Intents Bridge API URL [default: https://bridge.chaindefuser.com/rpc]

    --dry-run
        Log actions without executing transactions

    --near-account <ACCOUNT>
        NEAR account holding treasury funds

    --near-signer-key <KEY>
        NEAR private key (ed25519:...)

    --eth-enabled
        Enable Ethereum treasury (feature-gated, requires "ethereum" feature)
```

### Environment Variables

All CLI arguments can be set via environment variables:

```bash
# Server configuration
PORT=3000
NETWORK=mainnet
BRIDGE_API_URL=https://bridge.chaindefuser.com/rpc
DRY_RUN=false

# NEAR treasury (required)
NEAR_ACCOUNT=treasury.near
NEAR_SIGNER_KEY="ed25519:..."
NEAR_RPC_URL=https://rpc.mainnet.near.org

# Ethereum deposits (optional)
ETH_PRIVATE_KEY=0x...
ETH_RPC_URL=https://eth.llamarpc.com

# Solana deposits (optional)
SOLANA_PRIVATE_KEY=...
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com

# Withdrawal destinations (required for withdrawals)
ETH_WITHDRAW_ADDRESS=0x...
ARBITRUM_WITHDRAW_ADDRESS=0x...
BASE_WITHDRAW_ADDRESS=0x...
OPTIMISM_WITHDRAW_ADDRESS=0x...
POLYGON_WITHDRAW_ADDRESS=0x...
SOLANA_WITHDRAW_ADDRESS=...
```

## Development

### Running Tests

```bash
# Run all tests (148 tests)
cargo nextest run -p templar-funding-bridge

# Or with standard cargo test
cargo test -p templar-funding-bridge

# Run only unit tests
cargo test -p templar-funding-bridge --lib

# Run only integration tests
cargo test -p templar-funding-bridge --test integration_test

# Run with test output
cargo test -p templar-funding-bridge -- --nocapture
```

Test coverage includes:
- Bridge API client (real API format validation)
- Token decimal conversions
- Chain handler selection and routing
- HTTP endpoint responses
- Operation tracking
- Integration tests with NEAR sandbox

### Test Coverage

```bash
# Generate coverage report
cargo llvm-cov --package templar-funding-bridge --html

# Open coverage report
open target/llvm-cov/html/index.html
```

### Building for Production

```bash
# Optimized release build
cargo build --release -p templar-funding-bridge

# Binary location
./target/release/funding-bridge
```

### Development Server

```bash
# Run with debug logging
RUST_LOG=debug cargo run -p templar-funding-bridge -- \
  --dry-run \
  --near-account test.near \
  --near-signer-key "ed25519:..."
```

## Architecture

### High-Level Design

```
┌─────────────┐
│   Client    │ (CLI, Scanner, Liquidator, etc.)
└──────┬──────┘
       │ HTTP/JSON
       ▼
┌─────────────────────────────────────┐
│   Funding Bridge (REST API)         │
│                                     │
│  ┌──────────────────────────────┐  │
│  │   FundingManager             │  │
│  │   - Smart chain selection    │  │
│  │   - Priority routing         │  │
│  │   - Balance checking         │  │
│  └───────────┬──────────────────┘  │
│              │                      │
│       ┌──────┴───────┐              │
│       ▼              ▼              │
│  ChainHandlers   OperationTracker  │
│  - NEAR          - In-memory state │
│  - Ethereum*     - Status queries  │
│  (*feature-gated)                  │
└───────┬────────────────────────────┘
        │
        ▼
  NEAR Protocol
  (Mainnet/Testnet)
```

### Key Components

- **FundingManager** - Orchestrates multi-chain operations, chain selection logic
- **ChainHandler** - Trait for chain-specific operations (balance, transfers)
- **NearHandler** - NEAR blockchain integration via JSON-RPC
- **BridgeClient** - NEAR Intents Bridge API wrapper with caching
- **TokenRegistry** - Token info caching and decimal conversion utilities
- **OperationTracker** - In-memory operation status tracking
- **REST Routes** - Axum handlers for HTTP endpoints
- **Metrics** - Prometheus metrics for observability

### Module Structure

```
src/
├── app.rs          # Application state initialization
├── bridge/         # NEAR Intents Bridge API client
│   ├── client.rs   # HTTP client with token caching
│   └── models.rs   # ChainId, TokenInfo, API types
├── chain/          # Chain handlers
│   ├── near.rs     # NEAR Protocol handler
│   └── ethereum.rs # Ethereum handler
├── manager/        # Multi-chain orchestration
├── routes/         # HTTP endpoints
│   ├── deposit.rs  # Deposit flow
│   ├── withdraw.rs # Withdrawal with multi-chain support
│   ├── health.rs   # Health checks with bridge status
│   └── metrics.rs  # Prometheus metrics
├── tokens.rs       # Token registry and decimal handling
├── metrics.rs      # Metrics collection
└── tracker.rs      # Operation tracking
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed design documentation.

## Deployment

### Docker (Recommended)

```dockerfile
FROM rust:1.85 as builder
WORKDIR /app
COPY . .
RUN cargo build --release -p templar-funding-bridge

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/funding-bridge /usr/local/bin/
ENTRYPOINT ["funding-bridge"]
```

Build and run:

```bash
docker build -t funding-bridge .
docker run -p 3000:3000 \
  -e NEAR_ACCOUNT=treasury.near \
  -e NEAR_SIGNER_KEY="ed25519:..." \
  funding-bridge
```

### Systemd Service

```ini
[Unit]
Description=Funding Bridge Service
After=network.target

[Service]
Type=simple
User=funding-bridge
WorkingDirectory=/opt/funding-bridge
ExecStart=/opt/funding-bridge/funding-bridge \
  --near-account treasury.near \
  --near-signer-key "ed25519:..."
Restart=always
RestartSec=10

Environment=RUST_LOG=info
Environment=NETWORK=mainnet

[Install]
WantedBy=multi-user.target
```

### Health Monitoring

```bash
# Health check endpoint for monitoring systems
curl -f http://localhost:3000/health || exit 1

# Prometheus metrics
curl http://localhost:3000/metrics

# Check bridge API connectivity
curl -s http://localhost:3000/health | jq '.bridge_api_status'
```

## Security Considerations

### Private Key Management

**IMPORTANT:** Never commit private keys to version control.

Production recommendations:
- Use environment variables or secure secrets management (HashiCorp Vault, AWS Secrets Manager)
- Consider NEAR account with limited permissions (separate hot wallet)
- Rotate keys regularly
- Monitor transaction activity

### Network Security

- Run behind reverse proxy (nginx, Caddy) with TLS
- Use firewall rules to restrict access
- Consider API authentication for production
- Rate limit requests

### Gas and Storage

- Service pays gas fees for NEAR transactions from treasury account
- Monitor treasury balance to ensure sufficient NEAR for gas
- NEP-141 storage deposits are automatically handled

## Troubleshooting

### Service Won't Start

```bash
# Check configuration
./target/release/funding-bridge --help

# Validate NEAR key format
echo $NEAR_SIGNER_KEY | grep -E '^ed25519:[a-zA-Z0-9]{88}$'

# Test RPC connectivity
curl -s https://rpc.testnet.near.org/status

# Run with debug logging
RUST_LOG=debug ./target/release/funding-bridge ...
```

### Deposit Returns INSUFFICIENT

- Check treasury account balance: `near view-state <treasury-account> --finality final`
- Verify asset identifier matches contract ID
- Check RPC connectivity and response times

### Transaction Fails

- Ensure treasury account has sufficient NEAR for gas (~0.01 NEAR per transaction)
- Verify signer key matches treasury account
- Check storage deposits for NEP-141 tokens
- Review transaction error in NEAR Explorer

## Contributing

See the main [CONTRIBUTING.md](../../CONTRIBUTING.md) for contribution guidelines.

## License

GPL-3.0 - See [LICENSE](LICENSE) for details.

## Links

- [NEAR Protocol](https://near.org)
- [NEAR Intents Bridge](https://docs.near-intents.org/)
- [Templar Protocol Documentation](https://docs.templar.finance)
- [Bug Reports](https://github.com/templar-protocol/contracts/issues)

---

**Version:** 0.1.0
**Last Updated:** 2025-11-14
