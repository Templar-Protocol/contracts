# Templar Liquidator Bot - Docker Management
# Makefile for container operations

.PHONY: help build run stop logs clean dev prod test

# Default target
.DEFAULT_GOAL := help

# Variables
IMAGE_NAME := templar-liquidator
IMAGE_TAG := latest
CONTAINER_NAME := templar-liquidator

# Help command
help: ## Show this help message
	@echo "Templar Liquidator Bot - Docker Commands"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-15s %s\n", $$1, $$2}'

# Build Docker image
build: ## Build the Docker image
	@echo "Building Docker image..."
	docker build -t $(IMAGE_NAME):$(IMAGE_TAG) -f Dockerfile ../..
	@echo "✓ Image built: $(IMAGE_NAME):$(IMAGE_TAG)"

# Build with no cache
build-no-cache: ## Build Docker image without cache
	@echo "Building Docker image (no cache)..."
	docker build --no-cache -t $(IMAGE_NAME):$(IMAGE_TAG) -f Dockerfile ../..
	@echo "✓ Image built: $(IMAGE_NAME):$(IMAGE_TAG)"

# Run container in development mode (dry-run)
dev: ## Run liquidator in development mode (dry-run)
	@echo "Starting liquidator in development mode..."
	docker-compose up -d
	@echo "✓ Container started. View logs with: make logs"

# Run container in production mode
prod: ## Run liquidator in production mode
	@echo "Starting liquidator in production mode..."
	@echo "⚠️  WARNING: This will execute real liquidations!"
	@read -p "Continue? (yes/no): " confirm; \
	if [ "$$confirm" = "yes" ]; then \
		docker-compose -f docker-compose.prod.yml up -d; \
		echo "✓ Production container started"; \
	else \
		echo "Cancelled"; \
	fi

# Stop container
stop: ## Stop the running container
	@echo "Stopping liquidator..."
	docker-compose down || docker-compose -f docker-compose.prod.yml down || true
	@echo "✓ Container stopped"

# View logs
logs: ## View container logs (follow)
	docker-compose logs -f liquidator || docker logs -f $(CONTAINER_NAME)

# View logs (last 100 lines)
logs-tail: ## View last 100 lines of logs
	docker-compose logs --tail=100 liquidator || docker logs --tail=100 $(CONTAINER_NAME)

# Restart container
restart: stop dev ## Restart the container

# Shell into running container
shell: ## Open shell in running container
	docker exec -it $(CONTAINER_NAME) /bin/bash

# Clean up containers and images
clean: ## Remove containers and images
	@echo "Cleaning up..."
	docker-compose down -v || true
	docker-compose -f docker-compose.prod.yml down -v || true
	docker rmi $(IMAGE_NAME):$(IMAGE_TAG) || true
	@echo "✓ Cleaned up"

# Check container status
status: ## Check container status
	@echo "Container Status:"
	@docker ps -a --filter name=$(CONTAINER_NAME) --format "table {{.Names}}\t{{.Status}}\t{{.Image}}"

# Validate .env file exists
check-env: ## Check if .env file exists
	@if [ ! -f .env ]; then \
		echo "❌ Error: .env file not found"; \
		echo "   Copy .env.example to .env and configure:"; \
		echo "   cp .env.example .env"; \
		exit 1; \
	fi
	@echo "✓ .env file found"

# Run with environment check
run: check-env dev ## Check environment and run in dev mode

# Test build (multi-platform)
build-multiplatform: ## Build for multiple platforms (amd64, arm64)
	@echo "Building multi-platform image..."
	docker buildx build --platform linux/amd64,linux/arm64 \
		-t $(IMAGE_NAME):$(IMAGE_TAG) \
		-f Dockerfile ../.. \
		--load

# Health check
health: ## Check container health
	@docker inspect --format='{{.State.Health.Status}}' $(CONTAINER_NAME) 2>/dev/null || echo "Container not running"

# Show container resource usage
stats: ## Show container resource usage
	docker stats $(CONTAINER_NAME) --no-stream

# Export logs to file
export-logs: ## Export logs to liquidator.log
	docker logs $(CONTAINER_NAME) > liquidator.log 2>&1
	@echo "✓ Logs exported to liquidator.log"

# Quick test (build and run with dry-run)
test: build run ## Build and run in test mode
	@echo "✓ Test deployment complete"
	@echo "  View logs: make logs"
	@echo "  Stop: make stop"
