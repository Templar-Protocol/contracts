# Templar Market Monitor Deployment

## Prerequisites

1. **Hetzner Server** - Fresh Ubuntu 24.04+ server
2. **SSH Access** - Root or sudo access to the server
3. **Credentials Ready**:
   - Telegram bot token (from @BotFather)
   - Telegram channel/group ID
   - Optional: Telegram thread ID for group topics

## Deployment Steps

### 1. Initial Server Setup

SSH to your Hetzner server and run the initialization script:

```bash
# On the server (as root or with sudo)
curl -fsSL https://raw.githubusercontent.com/Templar-Protocol/contracts/feat/market-monitor/service/market-monitor/scripts/init-server.sh -o init-server.sh
chmod +x init-server.sh
./init-server.sh
```

This will:
- Install Docker and Docker Compose
- Create `monitor` user
- Set up app directory at `/opt/templar-market-monitor`
- Configure necessary permissions

### 2. Deploy from Local Machine

From your local machine (contracts repo):

```bash
cd service/market-monitor

# Deploy using git (recommended)
./scripts/deploy.sh <server-ip>

# Or build locally and transfer
./scripts/deploy.sh <server-ip> --build-local
```

### 3. Configure Environment

SSH to the server and edit the `.env` file:

```bash
ssh monitor@<server-ip>
cd /opt/templar-market-monitor/repo/service/market-monitor
nano .env
```

Required configuration:
```bash
# Registries
REGISTRY_ACCOUNT_IDS=v1.tmplr.near

# Telegram
TELEGRAM_BOT_TOKEN=your_bot_token_here
TELEGRAM_CHANNEL_ID=-1001234567890
# Optional for group threads:
# TELEGRAM_THREAD_ID=1234

# Scheduling (8 AM CST = 14:00 UTC)
SCAN_TIME=14:00

# Alerts
AT_RISK_THRESHOLD_PERCENT=10
MIN_POSITION_SIZE_USD=1000
```

### 4. Start Service

```bash
docker compose -f docker-compose.prod.yml up -d
```

## Management

### View Logs
```bash
cd /opt/templar-market-monitor/repo/service/market-monitor
docker compose -f docker-compose.prod.yml logs -f
```

### Restart Service
```bash
docker compose -f docker-compose.prod.yml restart
```

### Stop Service
```bash
docker compose -f docker-compose.prod.yml down
```

### Update Deployment
```bash
# From local machine
./scripts/deploy.sh <server-ip> --update
```

## Monitoring

### Check Service Status
```bash
docker compose -f docker-compose.prod.yml ps
```

### View Resource Usage
```bash
docker stats templar-market-monitor-prod
```

### Check Recent Logs
```bash
docker compose -f docker-compose.prod.yml logs --tail=100
```

## Telegram Setup

1. **Create Bot**: Message @BotFather → `/newbot`
2. **Get Channel ID**: Add @RawDataBot to your channel, it will show the ID
3. **For Group Threads**: Open thread in Telegram Web, URL shows thread ID
4. **Add Bot**: Make bot an admin with "Post Messages" permission

## Troubleshooting

### Service not starting
```bash
# Check logs
docker compose -f docker-compose.prod.yml logs

# Verify config
cat .env
```

### Connection issues
```bash
# Test RPC connection
docker compose -f docker-compose.prod.yml exec market-monitor curl https://free.rpc.fastnear.com
```

### Telegram not working
- Verify bot token is correct
- Check channel ID format (should start with `-100`)
- Ensure bot is added as admin with post permissions

## Resource Requirements

- **CPU**: 0.25-1 cores
- **Memory**: 256MB-1GB
- **Disk**: ~1GB for image + logs
- **Network**: Minimal (periodic NEAR RPC calls + Telegram)

## Security Notes

- Service runs as non-root `monitor` user
- Credentials stored in `.env` (not committed to git)
- Logs automatically rotated (50MB max, 5 files)
- Container resources limited in docker-compose.prod.yml
