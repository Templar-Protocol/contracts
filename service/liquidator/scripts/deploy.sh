#!/usr/bin/env bash
# Automated deployment script for Templar Liquidator to Hetzner
# Usage: ./deploy.sh <server-ip> [--build-local]

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SERVER_USER="liquidator"
APP_DIR="/opt/templar-liquidator"
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

Deploy Templar Liquidator to Hetzner server.

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
        log_error "  3. User 'liquidator' exists on server"
        exit 1
    fi
    
    log_info "SSH connection successful"
}

setup_server() {
    log_info "Setting up server environment..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << 'EOF'
        # Create app directory
        sudo mkdir -p /opt/templar-liquidator
        sudo chown -R liquidator:liquidator /opt/templar-liquidator
        
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
    make build
    
    log_info "Saving Docker image..."
    docker save templar-liquidator:latest | gzip > /tmp/templar-liquidator.tar.gz
    
    log_info "Transferring files to server..."
    
    # Transfer image
    scp /tmp/templar-liquidator.tar.gz ${SERVER_USER}@${SERVER_IP}:${APP_DIR}/
    
    # Transfer configs
    scp ${COMPOSE_FILE} ${SERVER_USER}@${SERVER_IP}:${APP_DIR}/docker-compose.yml
    scp .env.example ${SERVER_USER}@${SERVER_IP}:${APP_DIR}/
    
    # Clean up local temp file
    rm /tmp/templar-liquidator.tar.gz
    
    log_info "Loading image on server..."
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        cd ${APP_DIR}
        docker load < templar-liquidator.tar.gz
        rm templar-liquidator.tar.gz
        echo "Image loaded successfully"
EOF
}

deploy_git() {
    log_info "Deploying via git..."
    
    BRANCH="${GIT_BRANCH:-main}"
    
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        cd /opt/templar-liquidator
        
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
        
        # Build liquidator
        cd service/liquidator
        echo "Building liquidator with docker compose..."
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
            cd repo/service/liquidator
            if [ ! -f .env ]; then
                cp .env.example .env
                echo "Created .env file from template"
                echo "IMPORTANT: Edit ${APP_DIR}/repo/service/liquidator/.env with your credentials!"
            else
                echo ".env file already exists, skipping"
            fi
        fi
EOF
}

start_service() {
    log_info "Starting liquidator service..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << EOF
        if [ "$BUILD_LOCAL" = true ]; then
            cd ${APP_DIR}
        else
            cd ${APP_DIR}/repo/service/liquidator
        fi
        
        # Start in dry-run mode first
        docker compose down 2>/dev/null || true
        docker compose up -d
        
        echo ""
        echo "Liquidator started in DRY-RUN mode"
        echo ""
        echo "Next steps:"
        echo "  1. Check logs: docker compose logs -f"
        echo "  2. Verify operation for 24h"
        echo "  3. Edit .env and set DRY_RUN=false"
        echo "  4. Restart: docker compose down && docker compose up -d"
EOF
}

update_deployment() {
    log_info "Updating existing deployment..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << 'EOF'
        if [ -d /opt/templar-liquidator/repo ]; then
            cd /opt/templar-liquidator/repo
            git pull origin main
            cd service/liquidator
            
            echo "Stopping liquidator..."
            docker compose down
            
            echo "Rebuilding..."
            make build
            
            echo "Starting updated liquidator..."
            docker compose up -d
            
            echo "Update complete. Check logs with: docker compose logs -f"
        else
            echo "Error: Repository not found. Use full deployment instead."
            exit 1
        fi
EOF
}

show_status() {
    log_info "Checking deployment status..."
    
    ssh ${SERVER_USER}@${SERVER_IP} << 'EOF'
        if [ -d /opt/templar-liquidator/repo/service/liquidator ]; then
            cd /opt/templar-liquidator/repo/service/liquidator
        else
            cd /opt/templar-liquidator
        fi
        
        echo "=== Container Status ==="
        docker compose ps
        
        echo ""
        echo "=== Recent Logs ==="
        docker compose logs --tail 20
        
        echo ""
        echo "=== Resource Usage ==="
        docker stats --no-stream templar-liquidator-prod 2>/dev/null || echo "Container not running"
EOF
}

# Parse arguments
if [ $# -lt 1 ]; then
    usage
fi

SERVER_IP=$1
shift

BUILD_LOCAL=false
GIT_DEPLOY=true
UPDATE_MODE=false

while [ $# -gt 0 ]; do
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

# Main deployment flow
main() {
    log_info "Starting deployment to ${SERVER_IP}..."
    echo ""
    
    check_requirements
    test_connection
    
    if [ "$UPDATE_MODE" = true ]; then
        update_deployment
        show_status
    else
        setup_server
        
        if [ "$BUILD_LOCAL" = true ]; then
            deploy_local_build
        else
            deploy_git
        fi
        
        configure_env
        start_service
        
        echo ""
        log_info "Deployment complete!"
        echo ""
        echo "SSH into server: ssh ${SERVER_USER}@${SERVER_IP}"
        echo ""
        echo "View logs:"
        if [ "$BUILD_LOCAL" = true ]; then
            echo "  cd ${APP_DIR} && docker compose logs -f"
        else
            echo "  cd ${APP_DIR}/repo/service/liquidator && docker compose logs -f"
        fi
        echo ""
        log_warn "IMPORTANT: Configure .env with your credentials before production use!"
        echo ""
    fi
}

# Run main
main
