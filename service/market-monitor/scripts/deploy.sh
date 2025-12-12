#!/usr/bin/env bash
# Automated deployment script for Templar Market Monitor to Hetzner
# Usage: ./deploy.sh <server-ip> [--build-local]

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SERVER_USER="monitor"
APP_DIR="/opt/templar-market-monitor"
COMPOSE_FILE="docker-compose.prod.yml"

# Functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

usage() {
    cat << EOF
Usage: $0 <server-ip> [options]

Deploy Templar Market Monitor to Hetzner server.

Options:
    --build-local       Build Docker image locally and transfer
    --git-deploy        Deploy using git clone and build on server (default)
    --update            Update existing deployment
    -h, --help          Show this help message

Examples:
    $0 123.45.67.89                    # Git deploy (default)
    $0 123.45.67.89 --build-local      # Build locally and transfer
    $0 123.45.67.89 --update           # Update existing deployment

EOF
    exit 1
}

check_requirements() {
    log_info "Checking requirements..."
    
    if ! command -v ssh &> /dev/null; then
        log_error "ssh not found. Please install openssh client."
        exit 1
    fi
    
    if ! command -v scp &> /dev/null; then
        log_error "scp not found. Please install openssh client."
        exit 1
    fi
    
    if [ "$BUILD_LOCAL" = true ] && ! command -v docker &> /dev/null; then
        log_error "Docker not found. Please install Docker or use --git-deploy."
        exit 1
    fi
}

test_connection() {
    log_info "Testing SSH connection to $SERVER_IP..."
    
    if ! ssh -o ConnectTimeout=5 -o BatchMode=yes ${SERVER_USER}@${SERVER_IP} "echo 'Connection successful'" &> /dev/null; then
        log_error "Cannot connect to ${SERVER_USER}@${SERVER_IP}"
        log_error "Please ensure:"
        log_error "  1. Server IP is correct"
        log_error "  2. SSH key is configured"
        log_error "  3. User 'monitor' exists on server"
        exit 1
    fi
    
    log_info "SSH connection successful"
}

setup_server() {
    log_info "Setting up server environment..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << 'EOF'
        # Create app directory
        sudo mkdir -p /opt/templar-market-monitor
        sudo chown -R monitor:monitor /opt/templar-market-monitor
        
        # Check Docker installation
        if ! command -v docker &> /dev/null; then
            echo "Docker not found. Please run initial server setup first."
            exit 1
        fi
        
        echo "Server environment ready"
EOF
}

deploy_local_build() {
    log_info "Building Docker image locally..."
    
    # Build image
    cd "$(dirname "$0")/.."
    docker compose -f docker-compose.prod.yml build
    
    log_info "Saving Docker image..."
    docker save templar-market-monitor:latest | gzip > /tmp/templar-market-monitor.tar.gz
    
    log_info "Transferring files to server..."
    
    # Transfer image
    scp /tmp/templar-market-monitor.tar.gz ${SERVER_USER}@${SERVER_IP}:${APP_DIR}/
    
    # Transfer configs
    scp ${COMPOSE_FILE} ${SERVER_USER}@${SERVER_IP}:${APP_DIR}/docker-compose.yml
    scp .env.example ${SERVER_USER}@${SERVER_IP}:${APP_DIR}/
    
    # Clean up local temp file
    rm /tmp/templar-market-monitor.tar.gz
    
    log_info "Loading image on server..."
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        cd ${APP_DIR}
        docker load < templar-market-monitor.tar.gz
        rm templar-market-monitor.tar.gz
        echo "Image loaded successfully"
EOF
}

deploy_git() {
    log_info "Deploying via git..."
    
    BRANCH="${GIT_BRANCH:-feat/market-monitor}"
    
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        cd /opt/templar-market-monitor
        
        # Clone or update repository
        if [ -d "repo" ]; then
            echo "Updating existing repository..."
            cd repo
            git fetch origin
            git checkout ${BRANCH}
            git pull origin ${BRANCH}
        else
            echo "Cloning repository..."
            git clone -b ${BRANCH} https://github.com/Templar-Protocol/contracts.git repo
            cd repo
        fi
        
        # Build market-monitor
        cd service/market-monitor
        echo "Building market-monitor with docker compose..."
        docker compose -f docker-compose.prod.yml build
        
        echo "Git deployment complete"
EOF
}

configure_env() {
    log_info "Configuring environment..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        cd ${APP_DIR}
        
        if [ "$BUILD_LOCAL" = true ]; then
            if [ ! -f .env ]; then
                cp .env.example .env
                echo "Created .env file from template"
                echo "IMPORTANT: Edit ${APP_DIR}/.env with your credentials!"
            else
                echo ".env file already exists, skipping"
            fi
        else
            cd repo/service/market-monitor
            if [ ! -f .env ]; then
                cp .env.example .env
                echo "Created .env file from template"
                echo "IMPORTANT: Edit ${APP_DIR}/repo/service/market-monitor/.env with your credentials!"
            else
                echo ".env file already exists, skipping"
            fi
        fi
EOF
}

start_service() {
    log_info "Starting market-monitor service..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        if [ "$BUILD_LOCAL" = true ]; then
            cd ${APP_DIR}
        else
            cd ${APP_DIR}/repo/service/market-monitor
        fi
        
        # Stop existing container
        docker compose -f docker-compose.prod.yml down 2>/dev/null || true
        
        # Start service
        docker compose -f docker-compose.prod.yml up -d
        
        echo "Service started"
EOF
}

show_logs() {
    log_info "Showing recent logs..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        if [ "$BUILD_LOCAL" = true ]; then
            cd ${APP_DIR}
        else
            cd ${APP_DIR}/repo/service/market-monitor
        fi
        
        docker compose -f docker-compose.prod.yml logs --tail=50
EOF
}

main() {
    # Parse arguments
    if [ $# -eq 0 ]; then
        usage
    fi
    
    SERVER_IP=$1
    shift
    
    BUILD_LOCAL=false
    GIT_DEPLOY=true
    UPDATE_MODE=false
    
    while [[ $# -gt 0 ]]; do
        case $1 in
            --build-local)
                BUILD_LOCAL=true
                GIT_DEPLOY=false
                shift
                ;;
            --git-deploy)
                GIT_DEPLOY=true
                BUILD_LOCAL=false
                shift
                ;;
            --update)
                UPDATE_MODE=true
                shift
                ;;
            -h|--help)
                usage
                ;;
            *)
                log_error "Unknown option: $1"
                usage
                ;;
        esac
    done
    
    # Run deployment
    log_info "Starting deployment to $SERVER_IP"
    log_info "Deployment method: $([ "$BUILD_LOCAL" = true ] && echo "Local build" || echo "Git deploy")"
    
    check_requirements
    test_connection
    
    if [ "$UPDATE_MODE" = false ]; then
        setup_server
    fi
    
    if [ "$BUILD_LOCAL" = true ]; then
        deploy_local_build
    else
        deploy_git
    fi
    
    configure_env
    start_service
    
    log_info ""
    log_info "✅ Deployment completed successfully!"
    log_info ""
    log_info "Next steps:"
    if [ "$BUILD_LOCAL" = true ]; then
        log_info "  1. SSH to server: ssh ${SERVER_USER}@${SERVER_IP}"
        log_info "  2. Edit .env: cd ${APP_DIR} && nano .env"
    else
        log_info "  1. SSH to server: ssh ${SERVER_USER}@${SERVER_IP}"
        log_info "  2. Edit .env: cd ${APP_DIR}/repo/service/market-monitor && nano .env"
    fi
    log_info "  3. Restart service: docker compose -f docker-compose.prod.yml restart"
    log_info "  4. View logs: docker compose -f docker-compose.prod.yml logs -f"
    log_info ""
    
    show_logs
}

main "$@"
