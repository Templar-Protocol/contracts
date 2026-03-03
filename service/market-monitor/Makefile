.PHONY: help build run stop logs test restart clean

help:
	@echo "Templar Market Monitor"
	@echo ""
	@echo "Available commands:"
	@echo "  make build    - Build Docker image"
	@echo "  make run      - Start service in background"
	@echo "  make stop     - Stop service"
	@echo "  make restart  - Restart service"
	@echo "  make logs     - View logs (follow mode)"
	@echo "  make test     - Run test scan (dry-run, once)"
	@echo "  make clean    - Remove containers and images"

build:
	@echo "Building Docker image..."
	docker-compose build

run:
	docker-compose up -d
	@echo "Market monitor started"
	@echo "View logs with: make logs"

stop:
	docker-compose down

restart: stop run

logs:
	docker-compose logs -f

test:
	@echo "Running test scan..."
	cargo run --release

clean:
	docker-compose down -v
	docker rmi templar-market-monitor 2>/dev/null || true
	@echo "Cleanup complete"
