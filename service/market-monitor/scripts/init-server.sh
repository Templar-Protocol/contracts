#!/usr/bin/env bash
# Server initialization script for Templar Market Monitor
# Run this once on a fresh Hetzner server to set up the environment

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}Setting up Templar Market Monitor server...${NC}"

# Update system
echo "Updating system packages..."
sudo apt-get update
sudo apt-get upgrade -y

# Install Docker if not present
if ! command -v docker &> /dev/null; then
    echo "Installing Docker..."
    curl -fsSL https://get.docker.com -o get-docker.sh
    sudo sh get-docker.sh
    rm get-docker.sh
else
    echo "Docker already installed"
fi

# Install Docker Compose if not present
if ! command -v docker &> /dev/null || ! docker compose version &> /dev/null; then
    echo "Installing Docker Compose plugin..."
    sudo apt-get install -y docker-compose-plugin
else
    echo "Docker Compose already installed"
fi

# Create monitor user if it doesn't exist
if ! id "monitor" &>/dev/null; then
    echo "Creating monitor user..."
    sudo useradd -m -s /bin/bash monitor
    sudo usermod -aG docker monitor
    
    # Set up SSH for monitor user
    sudo mkdir -p /home/monitor/.ssh
    sudo cp ~/.ssh/authorized_keys /home/monitor/.ssh/ 2>/dev/null || true
    sudo chown -R monitor:monitor /home/monitor/.ssh
    sudo chmod 700 /home/monitor/.ssh
    sudo chmod 600 /home/monitor/.ssh/authorized_keys 2>/dev/null || true
else
    echo "User 'monitor' already exists"
    sudo usermod -aG docker monitor
fi

# Create app directory
echo "Setting up application directory..."
sudo mkdir -p /opt/templar-market-monitor
sudo chown -R monitor:monitor /opt/templar-market-monitor

# Install additional utilities
echo "Installing utilities..."
sudo apt-get install -y git curl htop nano

# Enable Docker service
sudo systemctl enable docker
sudo systemctl start docker

echo -e "${GREEN}✅ Server setup complete!${NC}"
echo ""
echo "Next steps:"
echo "  1. Copy your SSH key for the monitor user (if not done)"
echo "  2. Run deployment script from your local machine:"
echo "     ./scripts/deploy.sh <server-ip>"
