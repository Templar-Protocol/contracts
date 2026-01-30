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
# Soroban deploy (requires soroban CLI + compiled wasm)
# --------------------------------------------------------------------

soroban-deploy:
	@if [ -z "${SOROBAN_WASM:-}" ]; then echo "Set SOROBAN_WASM=/path/to/contract.wasm"; exit 1; fi
	@network="${SOROBAN_NETWORK:-testnet}"; \
	source="${SOROBAN_SOURCE:-identity}"; \
	echo "Deploying ${SOROBAN_WASM} to ${network} (source=${source})"; \
	soroban contract deploy --wasm "${SOROBAN_WASM}" --network "${network}" --source "${source}"
