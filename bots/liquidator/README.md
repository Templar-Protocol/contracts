# Templar Liquidator Bot

Production-grade liquidation bot for Templar Protocol. Monitors lending markets and liquidates under-collateralized positions.

## Quick Start

```bash
cp .env.example .env
nano .env  # Set LIQUIDATOR_ACCOUNT and LIQUIDATOR_PRIVATE_KEY
./scripts/run-mainnet.sh
```

## Configuration

**Required:** `LIQUIDATOR_ACCOUNT`, `LIQUIDATOR_PRIVATE_KEY` (in `.env`)

**Pre-configured:** Registry `v1.tmplr.near`, USDC asset, NEAR Intents swap (see [deployments.md](../../docs/src/deployments.md))

All options in `.env.example` with mainnet defaults.

## CLI Arguments

**Required:**
- `--registries` - Registry contracts
- `--signer-key` - Private key (`ed25519:...`)
- `--signer-account` - NEAR account
- `--asset` - Liquidation asset (`nep141:<token>` or `nep245:<contract>:<token_id>`)
- `--swap` - Swap provider: `rhea-swap` or `near-intents`

**Optional:**
- `--network` - `testnet`/`mainnet` (default: `testnet`)
- `--dry-run` - Scan and log without executing (default: `true`)
- `--timeout` - RPC timeout seconds (default: `60`)
- `--interval` - Seconds between runs (default: `600`)
- `--registry-refresh-interval` - Registry refresh seconds (default: `3600`)
- `--concurrency` - Concurrent liquidations (default: `10`)
- `--partial-percentage` - Liquidation % 1-100 (default: `50`)
- `--min-profit-bps` - Min profit basis points (default: `50`)
- `--max-gas-percentage` - Max gas % (default: `10`)
- `--log-json` - JSON output (default: `false`)

## Features

- **Strategies**: Partial (default, 40-60% gas savings) or Full liquidation
- **Swap Providers**: RheaSwap (DEX) or NEAR Intents (cross-chain)
- **Profitability**: Validates gas costs + profit margin before execution
- **Monitoring**: Tracing framework with structured logging
- **Concurrent**: Configurable concurrency for high throughput
- **Version Detection**: Automatically skips outdated market contracts by checking code hash

## How It Works

1. Discovers markets from registries
2. Monitors borrower positions continuously
3. Fetches oracle prices (Pyth)
4. Validates liquidation profitability
5. Swaps assets if needed
6. Executes liquidation via `ft_transfer_call`

## Production Deployment

1. Test with dry-run: `DRY_RUN=true ./scripts/run-mainnet.sh` (default)
2. Fund account with USDC
3. Set `DRY_RUN=false` and `MIN_PROFIT_BPS=50-200` (0.5-2%)
4. Enable `LOG_JSON=true`

**Funding:** Transfer USDC to bot account. Balance shared across all markets. Swap collateral back to USDC as needed.

## Monitoring

**Log Levels:**
```bash
RUST_LOG=info,templar_liquidator=debug ./liquidator [...]
```

**JSON Output:**
```bash
./liquidator --log-json --registries v1.tmplr.near [...]
```

**Monitor:** Liquidations/hour, success rate, swap performance, RPC response times

## Contract Version Management

The bot automatically detects and skips incompatible market contracts by checking code hashes:

- **Compatible Hashes**: List in `src/lib.rs` `COMPATIBLE_CONTRACT_HASHES`
- **Supported Versions**:
  - `66koB114bcvVDAtiKK7fhkZNUwLSTr2P5W6GwSgpdbmA` - templar-alpha.near registry
  - `mnDdmVzCejRwe6J7v981vYixroptYJJuLAzLXYZB5YD` - v1.tmplr.near registry
  - `3wnUgNWhm9S7ku3bLH5mruogiBWAdpJXJCzKNonYXZrW` - Additional version
- **Behavior**: Markets with unlisted hashes are logged and skipped
- **Adding Support**: Add new hash to the array when contracts are upgraded or new registries added

This supports multiple contract versions across different registries without maintaining a blocklist.

## Swap Providers

**Rhea Finance:** `dclv2.ref-finance.near` - Concentrated liquidity, NEP-141 only, 0.2% default fee

**NEAR Intents:** `intents.near` - Cross-chain solver, 120+ assets, NEP-141 & NEP-245

## Scripts

- `./scripts/run-mainnet.sh` - Mainnet runner (observation mode by default)
- `./scripts/run-testnet.sh` - Testnet runner (observation mode by default)

## Testing

```bash
cargo test -p templar-liquidator
cargo llvm-cov --package templar-liquidator --lib --tests
```

Coverage: ~39% (88 tests, strategy-focused)

## Building

```bash
cargo build -p templar-liquidator --bin liquidator
cargo build --release -p templar-liquidator --bin liquidator
```

## Security

- Slippage protection on swaps
- Gas cost limits prevent unprofitable liquidations
- Balance validation before operations
- Timeout handling for stuck transactions
- Private keys via environment variables only

## Performance

- Concurrency: 10 concurrent liquidations
- Batching: 100 positions/page, 500 markets/registry
- Partial liquidations: 40-60% gas savings
- Early exit profitability checks
