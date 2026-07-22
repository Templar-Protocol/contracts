# Templar contract-mvp — project tasks. Run `just` to list recipes.

network_filter := 'test(/^requires_network_/)'
sandbox_filter := '''
(
    (
        kind(test) & (
            package(templar-market-contract)
            | package(templar-vault-contract)
            | package(templar-registry-contract)
            | package(templar-universal-account-contract)
            | package(templar-proxy-oracle-near-contract)
            | package(templar-lst-oracle-contract)
            | package(templar-funding-bridge)
            | package(templar-relayer)
            | package(templar-gateway-testing)
            | package(templar-liquidator)
            | package(templar-gateway-core)
        )
    )
    | (
        package(templar-gateway-service)
        & (test(/^rpc::tests::/) | test(/^gateway_service::tests::/))
    )
)
'''
fast_filter := 'not ' + network_filter + ' and not ' + sandbox_filter
sandbox_test_filter := 'not ' + network_filter + ' and ' + sandbox_filter

# Show available recipes.
default:
    @just --list

# Format SQL (standalone files + inline in Rust).
sql-fmt:
    sleek $(find . -name '*.sql' -not -path './target/*')
    ./script/sql-fmt-inline.pl $(find . -name '*.rs' -not -path './target/*')

# Format Rust + SQL.
fmt: sql-fmt
    cargo fmt

# Run the complete local suite with shared prerequisites established once.
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source ./script/postgres-up.sh
    just -- _test-fast {{ args }}
    just -- _test-sandbox {{ args }}

# Run the fast/default gate.
test-fast *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source ./script/postgres-up.sh
    just -- _test-fast {{ args }}

_test-fast *args:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo nextest run --ignore-default-filter \
        -E '{{ fast_filter }}' {{ args }}

# Run the node-backed gate against a pooled neard sandbox.
test-sandbox *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source ./script/postgres-up.sh
    just -- _test-sandbox {{ args }}

_test-sandbox *args:
    #!/usr/bin/env bash
    set -euo pipefail
    trap './script/sandbox-down.sh || true' EXIT
    source ./script/sandbox-up.sh
    caller_args=({{ args }})
    sandbox_package_args=()
    use_default_packages=true
    for arg in "${caller_args[@]}"; do
        case "$arg" in
            -p | -p?* | --package | --package=*)
                use_default_packages=false
                break
                ;;
        esac
    done
    if "$use_default_packages"; then
        sandbox_package_output="$(
            printf '%s\n' '{{ sandbox_filter }}' | python3 script/sandbox-packages.py
        )"
        mapfile -t sandbox_packages <<< "$sandbox_package_output"
        for package in "${sandbox_packages[@]}"; do
            sandbox_package_args+=(-p "$package")
        done
    fi
    cargo nextest run --profile sandbox --ignore-default-filter \
        "${sandbox_package_args[@]}" \
        -E '{{ sandbox_test_filter }}' "${caller_args[@]}"

# Start the out-of-band sandbox neard (prints its RPC url).
sandbox-up:
    ./script/sandbox-up.sh

# Stop the out-of-band sandbox neard.
sandbox-down:
    ./script/sandbox-down.sh

# Generate HTML coverage for the fast library-test cut.
coverage:
    ./script/coverage.sh html '{{ fast_filter }}'

# Generate LCOV coverage for CI.
coverage-lcov:
    ./script/coverage.sh lcov '{{ fast_filter }}'

# Build the docs.
docs:
    ./script/build-docs.sh
