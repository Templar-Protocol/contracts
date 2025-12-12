#!/usr/bin/env bash
# Quick management commands for Templar Market Monitor

SERVER_IP="${1:-your-server-ip}"
SERVER_USER="monitor"
APP_DIR="/opt/templar-market-monitor/repo/service/market-monitor"

GREEN='\033[0;32m'
NC='\033[0m'

if [ "$SERVER_IP" = "your-server-ip" ]; then
    echo "Usage: $0 <server-ip> <command>"
    echo ""
    echo "Commands:"
    echo "  logs       - View service logs"
    echo "  status     - Check service status"
    echo "  restart    - Restart service"
    echo "  stop       - Stop service"
    echo "  start      - Start service"
    echo "  update     - Update from git and rebuild"
    echo "  shell      - SSH to server"
    echo "  edit-env   - Edit .env configuration"
    exit 1
fi

COMMAND="${2:-logs}"

case $COMMAND in
    logs)
        echo -e "${GREEN}Showing logs...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP} "cd ${APP_DIR} && docker compose -f docker-compose.prod.yml logs -f"
        ;;
    status)
        echo -e "${GREEN}Checking status...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP} "cd ${APP_DIR} && docker compose -f docker-compose.prod.yml ps && docker stats templar-market-monitor-prod --no-stream"
        ;;
    restart)
        echo -e "${GREEN}Restarting service...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP} "cd ${APP_DIR} && docker compose -f docker-compose.prod.yml restart"
        ;;
    stop)
        echo -e "${GREEN}Stopping service...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP} "cd ${APP_DIR} && docker compose -f docker-compose.prod.yml down"
        ;;
    start)
        echo -e "${GREEN}Starting service...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP} "cd ${APP_DIR} && docker compose -f docker-compose.prod.yml up -d"
        ;;
    update)
        echo -e "${GREEN}Updating deployment...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP} << 'EOF'
cd /opt/templar-market-monitor/repo
git pull
cd service/market-monitor
docker compose -f docker-compose.prod.yml down
docker compose -f docker-compose.prod.yml build
docker compose -f docker-compose.prod.yml up -d
docker compose -f docker-compose.prod.yml logs --tail=50
EOF
        ;;
    shell)
        echo -e "${GREEN}Opening SSH connection...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP}
        ;;
    edit-env)
        echo -e "${GREEN}Editing .env file...${NC}"
        ssh ${SERVER_USER}@${SERVER_IP} "nano ${APP_DIR}/.env"
        ;;
    *)
        echo "Unknown command: $COMMAND"
        exit 1
        ;;
esac
