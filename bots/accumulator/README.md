# Templar Accumulator Bot

Self-contained accumulator bot for applying interest to Templar Protocol borrow positions on NEAR blockchain.

## Overview

The accumulator bot monitors lending markets and periodically applies accrued interest to all active borrow positions. This is a maintenance operation that keeps the protocol's interest calculations up-to-date.

## Architecture

The accumulator is a standalone, self-sufficient reference implementation with no external bot dependencies:

```
accumulator/
├── Cargo.toml                    # Package manifest
├── README.md                     # This file
├── accumulator.service           # Systemd service file
└── src/
    ├── lib.rs                   # Core Accumulator struct and logic
    ├── main.rs                  # Binary entry point with event loop
    └── rpc.rs                   # RPC utilities (self-contained)
```

## Features

- **Self-Sufficient**: No dependencies on other bots - copy this directory and use it standalone
- **Multi-Market**: Monitors multiple markets across multiple registries
- **Concurrent Processing**: Configurable concurrency for high throughput
- **Auto-Discovery**: Automatically discovers new markets from registries
- **Production-Ready**: Comprehensive error handling and structured logging

## Prerequisites

- Rust (install via rustup)
- NEAR account (for signing transactions)
- Access to NEAR RPC endpoint

## Building

```bash
# Build release version
cargo build --release -p templar-accumulator --bin accumulator

# Binary will be at: target/release/accumulator
```

## Usage

### Basic Example

```bash
accumulator \
    --registries registry.testnet \
    --signer-key ed25519:YOUR_PRIVATE_KEY \
    --signer-account accumulator-bot.testnet \
    --network testnet
```

### Production Example with Environment Variables

```bash
#!/bin/bash
# production-accumulator.sh

export REGISTRIES_ACCOUNT_IDS="templar-registry1.near templar-registry2.near"
export SIGNER_KEY="ed25519:YOUR_PRIVATE_KEY"
export SIGNER_ACCOUNT_ID="accumulator-production.near"
export NETWORK="mainnet"
export TIMEOUT="120"
export INTERVAL="600"
export REGISTRY_REFRESH_INTERVAL="3600"
export CONCURRENCY="10"
export RUST_LOG="info,templar_accumulator=debug"

./target/release/accumulator
```

## CLI Arguments

| Argument | Short | Env Variable | Default | Description |
|----------|-------|--------------|---------|-------------|
| `--registries` | `-r` | `REGISTRIES_ACCOUNT_IDS` | Required | Registry contracts (space-separated) |
| `--signer-key` | `-k` | `SIGNER_KEY` | Required | Private key (format: `ed25519:...`) |
| `--signer-account` | `-s` | `SIGNER_ACCOUNT_ID` | Required | NEAR account for signing |
| `--network` | `-n` | `NETWORK` | `testnet` | Network: `testnet` or `mainnet` |
| `--timeout` | `-t` | `TIMEOUT` | `60` | RPC timeout in seconds |
| `--interval` | `-i` | `INTERVAL` | `600` | Interval between runs (seconds) |
| `--registry-refresh-interval` | `-r` | `REGISTRY_REFRESH_INTERVAL` | `3600` | Market refresh interval (seconds) |
| `--concurrency` | `-c` | `CONCURRENCY` | `4` | Concurrent operations |

## How It Works

### Main Loop

1. **Market Discovery**: Fetches all deployed markets from specified registries
2. **Create Accumulators**: Creates an accumulator instance for each market
3. **Event Loop**: Uses `tokio::select!` to handle two timers:
   - **Registry Refresh Timer**: Discovers new markets periodically
   - **Accumulation Timer**: Runs accumulation on all markets

### Accumulation Process

For each market:
1. **Fetch Borrow Positions**: Queries `list_borrow_positions` from market contract (paginated, 100 per page)
2. **Process Concurrently**: Applies interest to each position with configured concurrency
3. **Execute Transaction**: Calls `apply_interest(account_id)` on market contract

### Transaction Details

```rust
// Transaction structure
{
    "receiver_id": market_contract,
    "actions": [{
        "FunctionCall": {
            "method_name": "apply_interest",
            "args": { "account_id": borrower },
            "gas": 300_000_000_000_000, // 300 TGas
            "deposit": 0
        }
    }]
}
```

## Deployment

### Systemd Service

A systemd service file is included: `accumulator.service`

1. **Copy binary to system location**:
```bash
sudo cp target/release/accumulator /usr/local/bin/
sudo chmod +x /usr/local/bin/accumulator
```

2. **Create environment file**:
```bash
sudo nano /etc/default/accumulator
```

Add your configuration:
```bash
REGISTRIES_ACCOUNT_IDS="registry1.near registry2.near"
SIGNER_KEY="ed25519:..."
SIGNER_ACCOUNT_ID="accumulator.near"
NETWORK="mainnet"
TIMEOUT="120"
INTERVAL="600"
REGISTRY_REFRESH_INTERVAL="3600"
CONCURRENCY="10"
RUST_LOG="info"
```

3. **Install and start service**:
```bash
sudo cp accumulator.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable accumulator
sudo systemctl start accumulator
```

4. **Check status**:
```bash
sudo systemctl status accumulator
sudo journalctl -u accumulator -f
```

## Monitoring

### Logging Levels

Set `RUST_LOG` environment variable:

```bash
# Production (minimal logs)
export RUST_LOG="info"

# Development (detailed logs)
export RUST_LOG="debug,templar_accumulator=trace"

# Full debugging
export RUST_LOG="trace"
```

### Key Logs to Monitor

```
INFO  accumulator: Starting accumulator bot with args: ...
INFO  accumulator: Refreshing registry deployments
INFO  accumulator: Found 23 deployments
INFO  accumulator: Running accumulation for market: market1.testnet
INFO  accumulator: Starting accumulation for market: market1.testnet
INFO  accumulator: Accumulation successful
ERROR accumulator: Accumulation failed: <error>
INFO  accumulator: Accumulation job done
```

### Metrics to Track

1. **Success Rate**: Ratio of successful to failed accumulations
2. **Market Coverage**: Number of markets being monitored
3. **Position Count**: Total positions processed per run
4. **Error Rate**: Frequency of RPC or transaction errors

## Error Handling

The accumulator is designed to be resilient:

- **Failed Accumulation**: Logs error and continues with other positions
- **Failed Registry Refresh**: Keeps using existing market list
- **RPC Errors**: Retries with exponential backoff (up to 5 seconds)
- **Transaction Timeout**: Waits up to configured timeout, then polls for status

Errors are logged but don't stop the bot - it continues processing other positions and markets.

## Performance Tuning

### Concurrency

- **Low (2-4)**: Conservative, lower RPC load
- **Medium (4-8)**: Balanced performance
- **High (8-16)**: Maximum throughput, higher RPC load

### Intervals

- **Accumulation Interval**: How often to apply interest (default: 600s = 10 min)
  - More frequent = more up-to-date interest
  - Less frequent = lower transaction costs

- **Registry Refresh**: How often to discover new markets (default: 3600s = 1 hour)
  - More frequent = faster discovery of new markets
  - Less frequent = lower RPC load

## Cost Considerations

Each `apply_interest` call costs gas. Estimate:
- Gas per call: ~300 TGas
- Cost per call: ~0.03 NEAR (varies with gas price)
- For 100 positions every 10 minutes: ~432 NEAR/day

Optimize by:
1. Increasing accumulation interval
2. Filtering positions that need updates (not implemented yet)
3. Batching multiple accounts (requires contract changes)

## Security

1. **Private Key Management**:
   - Use environment variables (never commit keys)
   - Restrict file permissions on env file: `chmod 600 /etc/default/accumulator`
   - Consider hardware wallets for mainnet

2. **Account Permissions**:
   - Accumulator account only needs: `apply_interest` permission
   - Keep minimum NEAR balance for gas (e.g., 10 NEAR)

3. **Monitoring**:
   - Alert on high error rates
   - Monitor account balance
   - Track unexpected behavior (missing markets, etc.)

## Troubleshooting

### No Accumulations Happening

- Check: Are there any borrow positions?
  ```bash
  near view market.testnet list_borrow_positions '{"offset": 0, "count": 10}'
  ```
- Check: Is bot running?
  ```bash
  systemctl status accumulator
  ```
- Check: Account balance sufficient?
  ```bash
  near state accumulator.testnet
  ```

### High Failure Rate

- Increase `--timeout` (default: 60s)
- Reduce `--concurrency` (default: 4)
- Check RPC endpoint health
- Review logs for specific errors

### Markets Not Discovered

- Verify registries are correct:
  ```bash
  near view registry.testnet list_deployments '{"offset": 0, "count": 10}'
  ```
- Check `--registry-refresh-interval` setting
- Review logs during refresh cycle

## Development

### Adding Custom Logic

The accumulator is designed as a reference implementation. You can extend it:

```rust
// In lib.rs, modify accumulate() method
pub async fn accumulate(&self, borrow: AccountId) -> anyhow::Result<()> {
    // Add your custom logic here
    // Example: Check if position needs accumulation
    let position = self.get_position(&borrow).await?;
    if position.last_update_timestamp + 3600 > current_timestamp() {
        return Ok(()); // Skip recent updates
    }

    // Execute accumulation
    // ...
}
```

### RPC Module

The `rpc.rs` module contains all blockchain interaction utilities:
- `view()` - Call view methods
- `send_tx()` - Send signed transactions
- `get_access_key_data()` - Fetch nonce and block hash
- `list_deployments()` - Paginated market fetching
- `Network` enum - Mainnet/testnet configuration

You can modify these utilities for your specific needs.

## License

MIT License - Same as Templar Protocol

## Support

For issues or questions about the accumulator bot:
1. Review inline code documentation in `src/lib.rs` and `src/rpc.rs`
2. Check systemd logs: `journalctl -u accumulator -f`
3. Verify configuration with `accumulator --help`
