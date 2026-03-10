CRATE := templar-vault-client
LIB_NAME := templar_vault_client
DIST_DIR := dist
PY_OUT := $(DIST_DIR)/python

MODE ?= debug

UNIFFI_BINDGEN := cargo run -p $(CRATE) --bin uniffi-bindgen --

ifeq ($(MODE),release)
	CARGO_BUILD_FLAGS := --release
	PROFILE := release
else
	CARGO_BUILD_FLAGS :=
	PROFILE := debug
endif

ifeq ($(OS),Windows_NT)
	DYLIB := ../../target/$(PROFILE)/$(LIB_NAME).dll
else
	UNAME_S := $(shell uname -s)
	ifeq ($(UNAME_S),Darwin)
		DYLIB := ../../target/$(PROFILE)/lib$(LIB_NAME).dylib
	else
		DYLIB := ../../target/$(PROFILE)/lib$(LIB_NAME).so
	endif
endif

.PHONY: clean gen python smoke-test smoke-test-view smoke-test-full

clean:
	rm -rf "$(DIST_DIR)"

gen:
	cargo run --features=uniffi/cli --bin uniffi-bindgen $@

python: $(PY_OUT)
	cargo build -p $(CRATE) $(CARGO_BUILD_FLAGS)
	$(UNIFFI_BINDGEN) generate --language python --library "$(DYLIB)" --out-dir "$(PY_OUT)" --no-format
	cp "$(DYLIB)" "$(PY_OUT)/"

# Basic smoke test (no env vars needed)
smoke-test: python
	PYTHONPATH="$(PY_OUT)" python3 smoke_test.py

# View-only integration tests (read-only, requires VAULT_ACCOUNT)
# Usage: make smoke-test-view VAULT_ACCOUNT=myvault.testnet
smoke-test-view: python
	@if [ -z "$(VAULT_ACCOUNT)" ]; then \
		echo "Usage: make smoke-test-view VAULT_ACCOUNT=<vault.testnet>"; \
		echo "Optional: UNDERLYING_TOKEN=<token.testnet>"; \
		exit 1; \
	fi
	@PYTHONPATH="$(PY_OUT)" \
	VAULT_ACCOUNT="$(VAULT_ACCOUNT)" \
	UNDERLYING_TOKEN="$(UNDERLYING_TOKEN)" \
	RPC_URL="$(or $(RPC_URL),https://rpc.testnet.fastnear.com)" \
	RPC_API_KEY="$(RPC_API_KEY)" \
	python3 smoke_test.py

# Full happy path integration tests (requires funded accounts)
# Usage: make smoke-test-full VAULT_ACCOUNT=<vault> USER_ACCOUNT=<acc> USER_SECRET_KEY=<key> ...
smoke-test-full: python
	@if [ -z "$(VAULT_ACCOUNT)" ] || [ -z "$(USER_SECRET_KEY)" ]; then \
		echo "Usage: make smoke-test-full \\"; \
		echo "  VAULT_ACCOUNT=<vault.testnet> \\"; \
		echo "  USER_ACCOUNT=<user.testnet> \\"; \
		echo "  USER_SECRET_KEY='ed25519:...' \\"; \
		echo "  [ALLOCATOR_ACCOUNT=<allocator.testnet>] \\"; \
		echo "  [ALLOCATOR_SECRET_KEY='ed25519:...']"; \
		exit 1; \
	fi
	@PYTHONPATH="$(PY_OUT)" \
	VAULT_ACCOUNT="$(VAULT_ACCOUNT)" \
	UNDERLYING_TOKEN="$(UNDERLYING_TOKEN)" \
	MARKET_ACCOUNT="$(MARKET_ACCOUNT)" \
	USER_ACCOUNT="$(USER_ACCOUNT)" \
	USER_SECRET_KEY="$(USER_SECRET_KEY)" \
	ALLOCATOR_ACCOUNT="$(ALLOCATOR_ACCOUNT)" \
	ALLOCATOR_SECRET_KEY="$(ALLOCATOR_SECRET_KEY)" \
	RPC_URL="$(or $(RPC_URL),https://rpc.testnet.fastnear.com)" \
	RPC_API_KEY="$(RPC_API_KEY)" \
	RUN_FULL_TESTS=1 \
	python3 smoke_test.py --full

$(PY_OUT):
	mkdir -p "$(PY_OUT)"
