# Templar Accumulator Bot

Self-contained bot for applying interest to Templar Protocol borrow positions on NEAR blockchain.

## Quick Start

```bash
cargo build --release -p templar-accumulator --bin accumulator

accumulator \
    --registries registry.testnet \
    --signer-key ed25519:YOUR_PRIVATE_KEY \
    --signer-account accumulator-bot.testnet \
    --network testnet
```

## CLI Arguments

| Argument | Env Variable | Default | Description |
|----------|--------------|---------|-------------|
| `--registries` | `REGISTRIES_ACCOUNT_IDS` | Required | Registry contracts (space-separated) |
| `--signer-key` | `SIGNER_KEY` | Required | Private key (`ed25519:...`) |
| `--signer-account` | `SIGNER_ACCOUNT_ID` | Required | NEAR account for signing |
| `--network` | `NETWORK` | `testnet` | Network: `testnet` or `mainnet` |
| `--timeout` | `TIMEOUT` | `60` | RPC timeout in seconds |
| `--interval` | `INTERVAL` | `600` | Interval between runs (seconds) |
| `--registry-refresh-interval` | `REGISTRY_REFRESH_INTERVAL` | `3600` | Market refresh interval (seconds) |
| `--concurrency` | `CONCURRENCY` | `4` | Concurrent operations |

## Features

- **Self-Sufficient**: No external bot dependencies - standalone reference implementation
- **Multi-Market**: Monitors multiple markets across registries
- **Concurrent**: Configurable concurrency for throughput
- **Auto-Discovery**: Automatically discovers new markets
- **Resilient**: Failed accumulations don't stop processing

## How It Works

1. Discovers markets from registries
2. Fetches borrow positions (paginated, 100/page)
3. Applies interest to each position concurrently
4. Calls `apply_interest(account_id)` on market contract (300 TGas)

## Production Deployment

### Using Environment Variables

```bash
#!/bin/bash
export REGISTRIES_ACCOUNT_IDS="registry1.near registry2.near"
export SIGNER_KEY="ed25519:..."
export SIGNER_ACCOUNT_ID="accumulator.near"
export NETWORK="mainnet"
export TIMEOUT="120"
export INTERVAL="600"
export CONCURRENCY="10"
export RUST_LOG="info,templar_accumulator=debug"

./target/release/accumulator
```

### Systemd Service

See `accumulator.service` file.

1. Copy binary: `sudo cp target/release/accumulator /usr/local/bin/`
2. Create env file: `/etc/default/accumulator`
3. Install service: `sudo cp accumulator.service /etc/systemd/system/`
4. Start: `sudo systemctl enable accumulator && sudo systemctl start accumulator`
5. Monitor: `sudo journalctl -u accumulator -f`

## Monitoring

**Log Levels:**
```bash
export RUST_LOG="info"                          # Production
export RUST_LOG="debug,templar_accumulator=trace"  # Development
```

**Key Metrics:**
- Success rate (successful/failed accumulations)
- Market coverage (number of markets monitored)
- Position count (positions processed per run)
- Error rate (RPC/transaction errors)

## Performance Tuning

**Concurrency:**
- Low (2-4): Conservative, lower RPC load
- Medium (4-8): Balanced
- High (8-16): Maximum throughput

**Intervals:**
- Accumulation: How often to apply interest (default 600s)
- Registry Refresh: How often to discover markets (default 3600s)

## Cost Considerations

**Gas Usage:**
- Each `apply_interest()` call uses ~3.3 TGas in average
- Cost per call: ~0.0003 NEAR
- View calls (fetching positions) are free

**Daily Cost Estimates:**
| Positions/Day | NEAR Cost | USD Cost ($5 NEAR) |
|---------------|-----------|---------------------|
| 100 | 0.03 | $0.15 |
| 1,000 | 0.3 | $1.50 |
| 10,000 | 3.0 | $15.00 |

**Notes:**
- Actual gas usage varies by position complexity
- NEAR refunds unused gas automatically

**Optimize:**
- Increase accumulation interval
- Filter positions needing updates (requires changes)
- Batch accounts (requires contract changes)

## Error Handling

- Failed accumulation: Logs error, continues processing
- Failed registry refresh: Uses existing market list
- RPC errors: Retries with exponential backoff (up to 5s)
- Transaction timeout: Waits then polls for status

## Security

- Use environment variables for private keys
- Restrict file permissions: `chmod 600 /etc/default/accumulator`
- Account needs minimal NEAR balance for gas (~10 NEAR)
- Monitor account balance and error rates

## Troubleshooting

**No accumulations:**
- Check borrow positions exist: `near contract call-function as-read-only market.testnet list_borrow_positions json-args '{"offset": 0, "count": 10}' network-config testnet now`
- Verify bot running: `systemctl status accumulator`
- Check balance: `near account view-account-summary accumulator.testnet network-config testnet now`

**High failure rate:**
- Increase `--timeout` (default: 60s)
- Reduce `--concurrency` (default: 4)
- Check RPC endpoint health

## Building

```bash
cargo build --release -p templar-accumulator --bin accumulator
# Binary at: target/release/accumulator
```

## Development

Self-contained reference implementation. Extend by modifying `src/lib.rs`:

```rust
pub async fn accumulate(&self, borrow: AccountId) -> anyhow::Result<()> {
    // Add custom logic (e.g., skip recent updates)
    // Execute accumulation
}
```

RPC utilities in `src/rpc.rs`: `view()`, `send_tx()`, `get_access_key_data()`, `list_deployments()`
