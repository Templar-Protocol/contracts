# Templar Liquidator Bot

Automated liquidation bot for Templar Protocol lending markets.

## What is a Liquidator?

Lending protocols allow users to borrow assets against collateral. When collateral value drops below required levels, positions become **under-collateralized** and risky for the protocol. Liquidators protect the protocol by:

1. **Monitoring** borrower positions continuously
2. **Identifying** under-collateralized positions (health factor < 1)
3. **Executing** liquidations by repaying debt and receiving collateral at a discount
4. **Maintaining** protocol solvency and protecting lenders

This bot uses an **inventory-based model**: it maintains balances of borrow assets, liquidates positions when profitable, receives collateral, and optionally rebalances inventory through automated swaps.

## Quick Start

### Docker (Recommended)

```bash
cp .env.example .env
nano .env  # Configure credentials
make build && make run
```

### Native

```bash
cp .env.example .env
nano .env
cargo run --release
```

## Configuration

See `.env.example` for all options.

### Required

```bash
SIGNER_ACCOUNT_ID=liquidator.near
SIGNER_KEY=ed25519:...
REGISTRY_ACCOUNT_IDS=v1.tmplr.near
```

### Liquidation

```bash
LIQUIDATION_STRATEGY=partial    # partial | full
PARTIAL_PERCENTAGE=50           # 1-100 (if partial)
MIN_PROFIT_BPS=50              # Minimum profit (basis points)
```

### Collateral Strategy

```bash
COLLATERAL_STRATEGY=hold  # hold | swap-to-primary | swap-to-borrow
# PRIMARY_ASSET=nep141:usdc.near  # Required for swap-to-primary
```

- **hold** - Keep collateral as received
- **swap-to-primary** - Convert all to specified asset
- **swap-to-borrow** - Route back to borrow assets

### Market Filtering

```bash
# Process only specific collateral assets
ALLOWED_COLLATERAL_ASSETS=nep141:btc.omft.near,nep141:wrap.near

# Ignore specific collateral assets
IGNORED_COLLATERAL_ASSETS=nep141:meta-pool.near
```

### Intervals

```bash
LIQUIDATION_SCAN_INTERVAL=600   # Seconds between scans
REGISTRY_REFRESH_INTERVAL=3600  # Seconds between registry updates
```

## Docker Commands

```bash
make build    # Build image
make run      # Run (dry-run mode)
make logs     # View logs
make stop     # Stop container
make help     # Show all commands
```

## Production Deployment

1. Configure `.env` with production credentials
2. Fund account with borrow assets for target markets
3. Test: `DRY_RUN=true make run && make logs`
4. Deploy: `DRY_RUN=false make prod`

## Building

```bash
cargo build --release
./target/release/liquidator --help
```

## Documentation

- [IMPLEMENTATION.md](./IMPLEMENTATION.md) - Architecture and development guide
- [.env.example](./.env.example) - Full configuration reference

## License

MIT
