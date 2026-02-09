set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Templar vault workflows (kernel + curator-primitives + NEAR + Soroban)

default:
	@just --list

# --------------------------------------------------------------------
# Core tests
# --------------------------------------------------------------------

kernel-test:
	cargo test -p templar-vault-kernel

kernel-prop:
	cargo test -p templar-vault-kernel --test property_tests

curator-test:
	cargo test -p templar-curator-primitives

curator-prop:
	cargo test -p templar-curator-primitives --test proptests

near-test:
	cargo test -p templar-vault-contract

soroban-test:
	cargo test -p templar-soroban-runtime

soroban-flows:
	cargo test -p templar-soroban-runtime --test flows

soroban-integration:
	cargo test -p templar-soroban-runtime --test integration_tests

soroban-prop:
	cargo test -p templar-soroban-runtime --test property_tests

vault-test: kernel-test curator-test near-test soroban-test

# --------------------------------------------------------------------
# Parity testing (property-focused)
# --------------------------------------------------------------------

parity:
	just kernel-prop
	just curator-prop
	just soroban-prop

# --------------------------------------------------------------------
# Formal verification (Kani) - do NOT run automatically
# --------------------------------------------------------------------

kani-kernel:
	cargo kani -p templar-vault-kernel --features kani

kani-curator:
	cargo kani -p templar-curator-primitives

# --------------------------------------------------------------------
# Gas profiling (from AGENTS.md)
# --------------------------------------------------------------------

gas-report:
	cargo run --example gas_report -p market
	cargo run --example gas_report -p vault

# --------------------------------------------------------------------
# NEAR build/check
# --------------------------------------------------------------------

near-check:
	cargo check -p templar-vault-contract --target wasm32-unknown-unknown

near-build:
	cargo near build non-reproducible-wasm --manifest-path contract/vault/near/Cargo.toml

# --------------------------------------------------------------------
# Soroban operations (requires soroban CLI)
#
# Configure via env vars (see contract/vault/soroban/.env.example):
#   SOROBAN_NETWORK, SOROBAN_SOURCE, SOROBAN_WASM,
#   SOROBAN_CONTRACT_ID, SOROBAN_ADMIN,
#   SHARE_TOKEN_NAME, SHARE_TOKEN_SYMBOL, SHARE_TOKEN_DECIMALS
# --------------------------------------------------------------------

# Common soroban CLI flags (DRY helper — not a recipe)
_soroban_net := "--network ${SOROBAN_NETWORK:-testnet} --source ${SOROBAN_SOURCE:-identity}"

soroban-deploy:
	@if [ -z "${SOROBAN_WASM:-}" ]; then echo "Set SOROBAN_WASM=/path/to/contract.wasm"; exit 1; fi
	@echo "Deploying ${SOROBAN_WASM} to ${SOROBAN_NETWORK:-testnet}"; \
	soroban contract deploy --wasm "${SOROBAN_WASM}" {{_soroban_net}}

soroban-invoke fn *args:
	@if [ -z "${SOROBAN_CONTRACT_ID:-}" ]; then echo "Set SOROBAN_CONTRACT_ID"; exit 1; fi
	soroban contract invoke --id "${SOROBAN_CONTRACT_ID}" {{_soroban_net}} -- "{{fn}}" {{args}}

soroban-extend-ttl:
	@if [ -z "${SOROBAN_CONTRACT_ID:-}" ]; then echo "Set SOROBAN_CONTRACT_ID"; exit 1; fi
	soroban contract extend --id "${SOROBAN_CONTRACT_ID}" {{_soroban_net}} --ledgers-to-extend 100000

# Set share token metadata (SEP-41).
# Env: SHARE_TOKEN_NAME, SHARE_TOKEN_SYMBOL, SHARE_TOKEN_DECIMALS
soroban-share-metadata:
	@if [ -z "${SOROBAN_CONTRACT_ID:-}" ]; then echo "Set SOROBAN_CONTRACT_ID"; exit 1; fi
	@name="${SHARE_TOKEN_NAME:-Templar Vault Share}"; \
	symbol="${SHARE_TOKEN_SYMBOL:-tvSHARE}"; \
	decimals="${SHARE_TOKEN_DECIMALS:-7}"; \
	echo "Setting share metadata: name=${name} symbol=${symbol} decimals=${decimals}"; \
	soroban contract invoke --id "${SOROBAN_CONTRACT_ID}" {{_soroban_net}} \
		-- set_metadata --name "${name}" --symbol "${symbol}" --decimals "${decimals}"
