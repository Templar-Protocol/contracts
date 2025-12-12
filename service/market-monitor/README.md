# Templar Market Monitor

Position health monitoring service for Templar Protocol lending markets on NEAR blockchain. Continuously scans all markets and sends Telegram alerts for positions at risk of liquidation.

## Features

- 🔍 **Automated Monitoring**: Periodic or daily scans of all registered markets
- 📊 **Health Classification**: Three-zone system for position risk assessment
- 📱 **Telegram Alerts**: Real-time notifications to channels or specific group threads
- 🎯 **Smart Filtering**: Ignore specific markets or collateral types
- 📈 **Comprehensive Reports**: Position details with CR, debt value, and MCR distance
- 🔄 **Reliable Delivery**: Automatic retry logic for rate limiting

## Alert Zones

- 🔴 **Liquidatable**: CR < MCR (immediate liquidation risk)
- 🟡 **At Risk**: MCR ≤ CR < MCR × (1 + threshold%) (approaching liquidation)
- 🟢 **Healthy**: CR ≥ MCR × (1 + threshold%) (not reported)

## Quick Start

```bash
cd service/market-monitor
cp .env.example .env
# Edit .env - set TELEGRAM_BOT_TOKEN and TELEGRAM_CHANNEL_ID
make build && make run
```

## Configuration

All configuration is done via environment variables in `.env`:

### Required Settings

```bash
# Registry contracts to scan for markets
REGISTRY_ACCOUNT_IDS=v1.tmplr.near

# Telegram bot credentials (get from @BotFather)
TELEGRAM_BOT_TOKEN=1234567890:ABCdefGHIjklMNOpqrsTUVwxyz
TELEGRAM_CHANNEL_ID=-1001234567890

# Optional: For posting to specific topic/thread within a group
# TELEGRAM_THREAD_ID=1234
```

### Scheduling

```bash
# For periodic scans: */N (every N minutes)
SCAN_TIME=*/5              # Every 5 minutes

# For daily scans: HH:MM (specific time in UTC)
# SCAN_TIME=00:00          # Daily at midnight UTC
```

### Alert Thresholds

```bash
# Threshold percentage above MCR to flag positions as "at risk"
AT_RISK_THRESHOLD_PERCENT=10     # 10% buffer above MCR

# Minimum position debt value (USD) to include in alert reports
MIN_POSITION_SIZE_USD=1000       # Filter out positions < $1000
```

### Filtering (Optional)

```bash
# Exclude specific collateral asset types (comma-separated)
IGNORED_COLLATERAL_ASSETS=nep141:meta-pool.near,nep141:linear-protocol.near

# Exclude specific market contracts (comma-separated)
IGNORED_MARKETS=test-market.v1.tmplr.near,old-market.v1.tmplr.near
```

### Network & Advanced

```bash
NETWORK=mainnet                              # or testnet
RPC_URL=https://free.rpc.fastnear.com       # NEAR RPC endpoint
RPC_TIMEOUT=30                               # seconds
RUST_LOG=info                                # debug, info, warn, error
```

## Telegram Report Format

Reports include a summary followed by detailed position information:

```
📊 TEMPLAR MARKETS REPORT
Date: 2025-12-11 14:30 UTC
At Risk Threshold: 10% above MCR | Min Position Display Size: $1000

📈 SUMMARY
Markets: 5 active, 2 ignored (7 total)
Total Positions: 156
  ├─ 🟢 Healthy: 149 (95.5%)
  ├─ 🟡 At Risk: 5 (3.2%)
  └─ 🔴 Liquidatable: 2 (1.3%)

At Risk Value: $125.40K
Liquidatable Value: $188.00K

🔴 LIQUIDATABLE (2 position(s))
Positions below liquidation MCR - URGENT

Market: usdc-btc.v1.tmplr.near
MCR Liquidation: 1.10 (110.00%)

  alice.near
  CR: 1.05 (105.00%) ↓ 4.55% below MCR
  Debt: $63.00K

🟡 AT RISK (5 position(s))
Positions approaching liquidation

Market: usdc-eth.v1.tmplr.near
MCR Liquidation: 1.15 (115.00%)

  bob.near
  CR: 1.18 (118.00%) ↑ 2.61% above MCR
  Debt: $45.50K
```

## Docker Deployment

### Build and Run

```bash
cd service/market-monitor

# Build the image
docker-compose build

# Start the service
docker-compose up -d

# View logs
docker-compose logs -f

# Stop the service
docker-compose down
```

### Management Commands

```bash
# Restart the service
docker-compose restart

# Check status
docker-compose ps

# View recent logs
docker-compose logs --tail=100 market-monitor
```

## Local Development

### Run without Docker

```bash
cd service/market-monitor
cp .env.example .env
# Edit .env with your configuration

# Run directly
cargo run

# Or build release binary
cargo build --release
./target/release/market-monitor
```

### Testing

```bash
# Check for warnings
cargo clippy --all-features -- -D warnings

# Build
cargo build

# Test locally with minimal config (logs to console, no Telegram)
# Leave TELEGRAM_BOT_TOKEN empty in .env
cargo run
```

## Architecture

### Components

- **Scheduler**: Manages scan timing (interval-based or daily at specific UTC time)
- **Scanner**: Fetches data from NEAR RPC:
  - Market deployments from registry contracts
  - Market configurations and version metadata (NEP-330)
  - Borrow positions (with pagination)
  - Oracle price feeds
- **Analyzer**: Calculates collateralization ratios and classifies positions into zones
- **Reporter**: Formats comprehensive Telegram reports with statistics and position details
- **Telegram**: Delivers reports with retry logic for rate limiting

### Error Handling

- Registry fetch failures are logged but don't stop the scan
- Markets with incompatible versions are skipped
- Individual position analysis errors are logged and skipped
- Telegram rate limits (HTTP 429) trigger automatic retry after 60s
- All scans complete successfully even with partial failures

## Getting Telegram Credentials

1. **Create a bot**: Message @BotFather on Telegram
   - Send `/newbot` and follow instructions
   - Copy the bot token (format: `1234567890:ABCdefGHIjklMNOpqrsTUVwxyz`)

2. **Get channel ID**:
   - Add @RawDataBot to your channel
   - It will display the channel ID (format: `-1001234567890`)
   - Remove @RawDataBot after getting the ID

3. **For group threads** (optional):
   - Open the specific thread/topic in Telegram Web
   - The URL contains the thread ID: `t.me/c/2452747383/4690`
   - Thread ID is the last number (`4690`)
   - Set both `TELEGRAM_CHANNEL_ID=-1002452747383` (starts from -100) and `TELEGRAM_THREAD_ID=4690`

4. **Add bot as admin**:
   - Go to channel/group settings → Administrators
   - Add your bot
   - Give it "Post Messages" permission
