# Templar Funding Bridge

.PHONY: help build start start-prod stop logs clean shell

.DEFAULT_GOAL := help

IMAGE := templar-funding-bridge
TAG := latest
COMPOSE := docker compose
ENV_FILE := .env

help: ## Show available commands
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Build Docker image
	docker build -t $(IMAGE):$(TAG) -f Dockerfile ../..

build-clean: ## Build without cache
	docker build --no-cache -t $(IMAGE):$(TAG) -f Dockerfile ../..

start: ## Start in dry-run mode
	$(COMPOSE) --env-file $(ENV_FILE) up -d

start-prod: ## Start in production mode
	$(COMPOSE) --env-file $(ENV_FILE) -f docker-compose.prod.yml up -d

stop: ## Stop all containers
	$(COMPOSE) down

restart: stop start ## Restart in dry-run mode

logs: ## Follow container logs
	$(COMPOSE) logs -f

logs-tail: ## Show last 100 log lines
	$(COMPOSE) logs --tail=100

shell: ## Open shell in container
	docker exec -it templar-funding-bridge /bin/bash

clean: ## Remove containers and images
	$(COMPOSE) down -v
	docker rmi $(IMAGE):$(TAG) 2>/dev/null || true

ps: ## Show container status
	$(COMPOSE) ps

stats: ## Show resource usage
	docker stats templar-funding-bridge --no-stream
