# Liquidator Deployment Guide

Complete guide for deploying and managing the Templar Liquidator service on a cloud server.

## Prerequisites

Before using these scripts, you need:

1. **A Cloud Server**
   - Ubuntu 24.04 LTS (recommended)
   - Minimum: 2 vCPU, 4GB RAM, 40GB SSD
   - Recommended providers: Hetzner, DigitalOcean, AWS, GCP
   - Public IP address

2. **SSH Access**
   - SSH key-based authentication configured
   - Root or sudo access to the server

3. **Local Development Machine**
   - SSH client installed
   - Git installed (for git-based deployment)
   - Docker installed (optional, for local builds)

## Quick Start

### 1. Server Setup (One-time)

SSH into your server as root and run the initialization script:

```bash
# Download and run server initialization
curl -fsSL https://raw.githubusercontent.com/Templar-Protocol/contracts/main/service/liquidator/scripts/init-server.sh | sudo bash
```

This script will:
- Install Docker and Docker Compose
- Create a `liquidator` user
- Configure firewall (UFW)
- Set up monitoring and log rotation
- Optimize system settings

### 2. SSH Key Setup

On your local machine, copy your SSH public key to the server:

```bash
# Copy SSH key to liquidator user
ssh-copy-id liquidator@YOUR_SERVER_IP
```

Test the connection:

```bash
ssh liquidator@YOUR_SERVER_IP
```

### 3. Deploy Liquidator

From your local machine in the liquidator directory:

```bash
# Deploy using git (recommended)
./scripts/deploy.sh YOUR_SERVER_IP

# Or deploy with local build
./scripts/deploy.sh YOUR_SERVER_IP --build-local
```

### 4. Configure Environment

SSH into the server and edit the `.env` file:

```bash
ssh liquidator@YOUR_SERVER_IP
cd /opt/templar-liquidator/repo/service/liquidator
nano .env
```

Required environment variables:
- `NEAR_ACCOUNT_ID` - Your NEAR account ID
- `NEAR_PRIVATE_KEY` - Your NEAR account private key (ed25519:...)
- `NETWORK` - `mainnet` or `testnet`
- `DRY_RUN` - Set to `true` for testing, `false` for production

### 5. Start in Dry-Run Mode

The liquidator starts automatically in dry-run mode. Monitor logs:

```bash
cd /opt/templar-liquidator/repo/service/liquidator
docker compose logs -f
```

Let it run for 24 hours to verify correct operation.

### 6. Enable Production Mode

After verifying dry-run mode works correctly:

```bash
# Edit .env and set DRY_RUN=false
nano .env

# Restart the service
docker compose down
docker compose up -d
```

## Scripts Overview

### `init-server.sh`

One-time server initialization script. Installs Docker, creates users, and configures the system.

**Usage:**
```bash
curl -fsSL https://raw.githubusercontent.com/Templar-Protocol/contracts/main/service/liquidator/scripts/init-server.sh | sudo bash
```

### `deploy.sh`

Automated deployment script. Handles building and deploying the liquidator.

**Usage:**
```bash
./deploy.sh <server-ip> [options]

Options:
  --git-deploy      Clone repo and build on server (default)
  --build-local     Build locally and transfer image
  --update          Update existing deployment
```

**Examples:**
```bash
# Initial deployment via git
./deploy.sh 123.45.67.89

# Deploy with local build
./deploy.sh 123.45.67.89 --build-local

# Update existing deployment
./deploy.sh 123.45.67.89 --update
```

### `setup-loki-grafana.sh`

Sets up Grafana + Loki for log aggregation and monitoring (optional but recommended).

**Usage:**
```bash
# SSH to server
ssh liquidator@YOUR_SERVER_IP

# Run setup
sudo bash /tmp/setup-loki-grafana.sh
```

**Access Grafana:**
- URL: `http://YOUR_SERVER_IP:3000`
- Default credentials: `admin` / `admin`
- Change password on first login

### `run-mainnet.sh` / `run-testnet.sh`

Quick local testing scripts for mainnet/testnet environments.

**Usage:**
```bash
# From service/liquidator directory
./scripts/run-mainnet.sh
# or
./scripts/run-testnet.sh
```

## Manual Deployment Steps

If you prefer manual deployment:

### 1. Server Setup

```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install Docker
curl -fsSL https://get.docker.com | sudo sh

# Install Docker Compose
sudo apt install docker-compose-plugin

# Create user
sudo useradd -m -s /bin/bash liquidator
sudo usermod -aG docker liquidator
```

### 2. Clone Repository

```bash
su - liquidator
mkdir -p /opt/templar-liquidator
cd /opt/templar-liquidator
git clone https://github.com/Templar-Protocol/contracts.git repo
cd repo/service/liquidator
```

### 3. Configure Environment

```bash
cp .env.example .env
nano .env  # Edit with your credentials
```

### 4. Build and Run

```bash
docker compose -f docker-compose.prod.yml build
docker compose -f docker-compose.prod.yml up -d
```

### 5. Monitor

```bash
docker compose logs -f
```

## Monitoring and Maintenance

### View Logs

```bash
# Real-time logs
docker compose logs -f

# Last 100 lines
docker compose logs --tail 100

# Search logs
docker compose logs | grep "liquidation"
```

### Check Status

```bash
docker compose ps
docker stats templar-liquidator-prod
```

### Restart Service

```bash
docker compose restart
```

### Update Deployment

```bash
cd /opt/templar-liquidator/repo
git pull origin main
cd service/liquidator
docker compose down
docker compose -f docker-compose.prod.yml build
docker compose up -d
```

## Firewall Configuration

If you install Grafana monitoring, open additional ports:

```bash
sudo ufw allow 3000/tcp  # Grafana UI
sudo ufw allow 3100/tcp  # Loki API (optional, can be localhost only)
sudo ufw reload
```

## Troubleshooting

### Container won't start

```bash
# Check logs
docker compose logs

# Check disk space
df -h

# Check Docker status
sudo systemctl status docker
```

### Out of memory

```bash
# Check memory usage
free -h
docker stats

# Consider upgrading server or reducing CPU limits in docker-compose.prod.yml
```

### Can't connect to server

```bash
# Test SSH connection
ssh -v liquidator@YOUR_SERVER_IP

# Check if SSH key is added
ssh-add -l

# Copy SSH key again
ssh-copy-id liquidator@YOUR_SERVER_IP
```

### Logs not showing in Grafana

```bash
# Check Promtail status
sudo systemctl status promtail

# Check Loki status
sudo systemctl status loki

# Check Grafana status
sudo systemctl status grafana-server

# View service logs
sudo journalctl -u promtail -n 50
```
