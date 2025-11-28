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
LIQUIDATION_STRATEGY=partial    # partial | full | fixed-amount
PARTIAL_LIQUIDATION_PERCENTAGE=50           # 1-100 (% of available funds to use)
FIXED_LIQUIDATION_AMOUNT=1000000000  # Token base units (e.g., 1000 USDC)
LOOP_LIQUIDATION=false          # Repeatedly liquidate until healthy
MAX_LOOP_ITERATIONS=10          # Safety limit for loop liquidation
MIN_PROFIT_BPS=50              # Minimum profit (basis points)
```

- **partial** - Use percentage of available funds per liquidation
- **full** - Use 100% of available funds up to liquidatable amount
- **fixed-amount** - Use a fixed amount per liquidation (ideal for loop liquidation)
- **loop_liquidation** - When enabled, continues liquidating the same position until it becomes healthy or runs out of funds
- **max_loop_iterations** - Safety limit to prevent infinite loops (default: 10)

### Collateral Strategy

```bash
COLLATERAL_STRATEGY=hold  # hold | swap-to-borrow
```

- **hold** - Keep collateral as received
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

**📖 For complete deployment guide, see [DEPLOYMENT.md](./DEPLOYMENT.md)**

### Quick Deploy to Cloud Server

```bash
# 1. Create Ubuntu 24.04 server (Hetzner, DigitalOcean, AWS, etc.)
#    Minimum: 2 vCPU, 4GB RAM, 40GB SSD

# 2. Initialize server (one-time setup)
curl -fsSL https://raw.githubusercontent.com/Templar-Protocol/contracts/main/service/liquidator/scripts/init-server.sh | sudo bash

# 3. Configure SSH access from your local machine
ssh-copy-id liquidator@YOUR_SERVER_IP

# 4. Deploy liquidator
cd service/liquidator
./scripts/deploy.sh YOUR_SERVER_IP

# 5. Configure environment
ssh liquidator@YOUR_SERVER_IP
cd /opt/templar-liquidator/repo/service/liquidator
nano .env  # Add your NEAR credentials

# 6. Monitor logs
docker compose logs -f
```

### Deployment Scripts

| Script | Purpose |
|--------|---------|
| **`scripts/init-server.sh`** | One-time server setup (Docker, users, firewall) |
| **`scripts/deploy.sh`** | Deploy or update liquidator |
| **`scripts/setup-loki-grafana.sh`** | Install log monitoring (Grafana + Loki) |
| **`scripts/run-mainnet.sh`** | Quick local mainnet test |
| **`scripts/run-testnet.sh`** | Quick local testnet test |

See **[DEPLOYMENT.md](./DEPLOYMENT.md)** for detailed documentation.

## Building

```bash
cargo build --release
./target/release/liquidator --help
```

## Documentation

- **[DEPLOYMENT.md](./DEPLOYMENT.md)** - Complete deployment guide and scripts documentation
- **[IMPLEMENTATION.md](./IMPLEMENTATION.md)** - Architecture and development guide
- **[.env.example](./.env.example)** - Full configuration reference

## Architecture

See [IMPLEMENTATION.md](./IMPLEMENTATION.md) for detailed architecture, development guide, and testing instructions.

## License

MIT
