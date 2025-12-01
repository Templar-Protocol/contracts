#!/usr/bin/env bash
#
# Grafana + Loki Log Aggregation Setup
# 
# This script installs and configures:
#   - Grafana Loki: Log aggregation backend (stores logs for 30 days)
#   - Promtail: Log collector (reads Docker container logs)
#   - Grafana: Visualization and query UI
#
# Requirements:
#   - Ubuntu 24.04 LTS
#   - Docker installed and running
#   - Root or sudo access
#
# Usage:
#   sudo bash setup-loki-grafana.sh
#
# After installation:
#   - Grafana UI: http://YOUR_SERVER_IP:3000
#   - Default credentials: admin / admin (change on first login)
#   - Loki API: http://localhost:3100
#
# Firewall configuration:
#   sudo ufw allow 3000/tcp  # Grafana UI
#   sudo ufw allow 3100/tcp  # Loki API (optional, can keep localhost-only)
#
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if running as root
if [ "$EUID" -ne 0 ]; then 
    log_error "Please run as root"
    exit 1
fi

log_info "Installing Grafana Loki + Grafana for liquidator monitoring..."

# Install prerequisites
log_info "Installing prerequisites..."
apt-get update -qq
apt-get install -y unzip wget curl gpg

# Create directories
mkdir -p /opt/loki
mkdir -p /var/lib/loki/{index,chunks}
mkdir -p /etc/loki
mkdir -p /etc/promtail

# Download Loki
log_info "Downloading Loki..."
cd /opt/loki
LOKI_VERSION="3.3.2"
wget -q https://github.com/grafana/loki/releases/download/v${LOKI_VERSION}/loki-linux-amd64.zip
unzip -o loki-linux-amd64.zip
chmod +x loki-linux-amd64
rm loki-linux-amd64.zip

# Download Promtail
log_info "Downloading Promtail..."
wget -q https://github.com/grafana/loki/releases/download/v${LOKI_VERSION}/promtail-linux-amd64.zip
unzip -o promtail-linux-amd64.zip
chmod +x promtail-linux-amd64
rm promtail-linux-amd64.zip

# Create Loki config
log_info "Creating Loki configuration..."
cat > /etc/loki/config.yaml <<'EOF'
auth_enabled: false

server:
  http_listen_port: 3100
  grpc_listen_port: 9096

common:
  path_prefix: /var/lib/loki
  storage:
    filesystem:
      chunks_directory: /var/lib/loki/chunks
      rules_directory: /var/lib/loki/rules
  replication_factor: 1
  ring:
    kvstore:
      store: inmemory

schema_config:
  configs:
    - from: 2020-10-24
      store: boltdb-shipper
      object_store: filesystem
      schema: v11
      index:
        prefix: index_
        period: 24h

ruler:
  alertmanager_url: http://localhost:9093

# Retention - keep 30 days
limits_config:
  retention_period: 720h
  enforce_metric_name: false
  reject_old_samples: true
  reject_old_samples_max_age: 168h
  ingestion_rate_mb: 10
  ingestion_burst_size_mb: 20

chunk_store_config:
  max_look_back_period: 0s

table_manager:
  retention_deletes_enabled: true
  retention_period: 720h

compactor:
  working_directory: /var/lib/loki/compactor
  shared_store: filesystem
  compaction_interval: 10m
  retention_enabled: true
  retention_delete_delay: 2h
  retention_delete_worker_count: 150
EOF

# Create Promtail config
log_info "Creating Promtail configuration..."
cat > /etc/promtail/config.yaml <<'EOF'
server:
  http_listen_port: 9080
  grpc_listen_port: 0

positions:
  filename: /var/lib/promtail/positions.yaml

clients:
  - url: http://localhost:3100/loki/api/v1/push

scrape_configs:
  - job_name: docker
    docker_sd_configs:
      - host: unix:///var/run/docker.sock
        refresh_interval: 5s
    relabel_configs:
      - source_labels: ['__meta_docker_container_name']
        regex: '/(.*)'
        target_label: 'container'
      - source_labels: ['__meta_docker_container_log_stream']
        target_label: 'stream'
    pipeline_stages:
      - docker: {}
      - regex:
          expression: '^(?P<timestamp>\S+)\s+(?P<level>\w+)\s+(?P<message>.*)$'
      - labels:
          level:
          
  - job_name: system
    static_configs:
      - targets:
          - localhost
        labels:
          job: varlogs
          __path__: /var/log/*log
EOF

# Create Promtail positions directory
mkdir -p /var/lib/promtail

# Set permissions
chown -R liquidator:liquidator /var/lib/loki
chown -R liquidator:liquidator /var/lib/promtail

# Create Loki systemd service
log_info "Creating Loki service..."
cat > /etc/systemd/system/loki.service <<'EOF'
[Unit]
Description=Loki - Log Aggregation System
Documentation=https://grafana.com/docs/loki/latest/
After=network-online.target

[Service]
Type=simple
User=liquidator
Group=liquidator
ExecStart=/opt/loki/loki-linux-amd64 -config.file=/etc/loki/config.yaml
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# Create Promtail systemd service
log_info "Creating Promtail service..."
cat > /etc/systemd/system/promtail.service <<'EOF'
[Unit]
Description=Promtail - Log Collector for Loki
Documentation=https://grafana.com/docs/loki/latest/
After=network-online.target

[Service]
Type=simple
User=root
Group=root
ExecStart=/opt/loki/promtail-linux-amd64 -config.file=/etc/promtail/config.yaml
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# Install Grafana
log_info "Installing Grafana..."
apt-get install -y apt-transport-https software-properties-common wget
mkdir -p /etc/apt/keyrings/
wget -q -O - https://apt.grafana.com/gpg.key | gpg --dearmor > /etc/apt/keyrings/grafana.gpg
echo "deb [signed-by=/etc/apt/keyrings/grafana.gpg] https://apt.grafana.com stable main" | tee /etc/apt/sources.list.d/grafana.list
apt-get update
apt-get install -y grafana

# Configure Grafana datasource
log_info "Configuring Grafana datasource..."
mkdir -p /etc/grafana/provisioning/datasources
cat > /etc/grafana/provisioning/datasources/loki.yaml <<'EOF'
apiVersion: 1

datasources:
  - name: Loki
    type: loki
    access: proxy
    url: http://localhost:3100
    isDefault: true
    editable: true
EOF

# Create dashboard for liquidator
log_info "Creating liquidator dashboard..."
mkdir -p /etc/grafana/provisioning/dashboards
cat > /etc/grafana/provisioning/dashboards/dashboard.yaml <<'EOF'
apiVersion: 1

providers:
  - name: 'default'
    orgId: 1
    folder: ''
    type: file
    disableDeletion: false
    updateIntervalSeconds: 10
    allowUiUpdates: true
    options:
      path: /var/lib/grafana/dashboards
EOF

mkdir -p /var/lib/grafana/dashboards
cat > /var/lib/grafana/dashboards/liquidator.json <<'EOF'
{
  "dashboard": {
    "title": "Templar Liquidator",
    "tags": ["liquidator", "templar"],
    "timezone": "browser",
    "panels": [
      {
        "id": 1,
        "title": "Liquidation Events",
        "type": "logs",
        "gridPos": {"x": 0, "y": 0, "w": 24, "h": 8},
        "targets": [
          {
            "expr": "{container=\"templar-liquidator\"} |~ \"liquidat\""
          }
        ]
      },
      {
        "id": 2,
        "title": "Errors",
        "type": "logs",
        "gridPos": {"x": 0, "y": 8, "w": 24, "h": 8},
        "targets": [
          {
            "expr": "{container=\"templar-liquidator\"} |~ \"ERROR|WARN\""
          }
        ]
      },
      {
        "id": 3,
        "title": "Market Scans",
        "type": "logs",
        "gridPos": {"x": 0, "y": 16, "w": 24, "h": 8},
        "targets": [
          {
            "expr": "{container=\"templar-liquidator\"} |~ \"Scanning market\""
          }
        ]
      }
    ],
    "schemaVersion": 16,
    "version": 0
  }
}
EOF

chown -R grafana:grafana /var/lib/grafana

# Configure firewall
log_info "Configuring firewall..."
ufw allow 3000/tcp comment 'Grafana'
ufw allow 3100/tcp comment 'Loki'

# Start services
log_info "Starting services..."
systemctl daemon-reload
systemctl enable loki promtail grafana-server
systemctl restart loki
sleep 2
systemctl restart promtail
systemctl restart grafana-server

# Wait for services to start
sleep 5

# Check status
log_info "Checking service status..."
systemctl is-active --quiet loki && log_info "✓ Loki is running" || log_error "✗ Loki failed to start"
systemctl is-active --quiet promtail && log_info "✓ Promtail is running" || log_error "✗ Promtail failed to start"
systemctl is-active --quiet grafana-server && log_info "✓ Grafana is running" || log_error "✗ Grafana failed to start"

# Get server IP
SERVER_IP=$(curl -s ifconfig.me)

log_info ""
log_info "=========================================="
log_info "Installation complete!"
log_info "=========================================="
log_info ""
log_info "Grafana URL: http://${SERVER_IP}:3000"
log_info "Default credentials:"
log_info "  Username: admin"
log_info "  Password: admin"
log_info ""
log_info "Loki endpoint: http://localhost:3100"
log_info ""
log_info "Next steps:"
log_info "  1. Open Grafana in your browser"
log_info "  2. Login with admin/admin (you'll be prompted to change password)"
log_info "  3. Go to Explore > Select 'Loki' datasource"
log_info "  4. Query: {container=\"templar-liquidator\"}"
log_info ""
log_info "Useful queries:"
log_info "  All liquidator logs:     {container=\"templar-liquidator\"}"
log_info "  Liquidation events:      {container=\"templar-liquidator\"} |~ \"liquidated=\""
log_info "  Errors only:             {container=\"templar-liquidator\"} |~ \"ERROR\""
log_info "  Specific market:         {container=\"templar-liquidator\"} |~ \"ibtc-usdc\""
log_info ""
log_info "Service management:"
log_info "  systemctl status loki"
log_info "  systemctl status promtail"
log_info "  systemctl status grafana-server"
log_info ""
log_info "Logs are retained for 30 days"
log_info "=========================================="
