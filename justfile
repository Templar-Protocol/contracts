# Templar contract-mvp — project tasks. Run `just` to list recipes.

set positional-arguments

network_filter := 'test(/^requires_network_/)'
sandbox_full_packages := trim('''
templar-market-contract
templar-vault-contract
templar-registry-contract
templar-universal-account-contract
templar-proxy-oracle-near-contract
templar-proxy-oracle-near-governance-contract
templar-lst-oracle-contract
templar-funding-bridge
templar-relayer
templar-gateway-testing
templar-liquidator
templar-gateway-core
''')
sandbox_gateway_package := 'templar-gateway-service'
sandbox_packages := sandbox_full_packages + "\n" + sandbox_gateway_package
sandbox_package_filter := 'package(' + replace(sandbox_full_packages, "\n", ') | package(') + ')'
sandbox_gateway_filter := 'package(' + sandbox_gateway_package + ') & (test(/^rpc::tests::/) | test(/^gateway_service::tests::/))'
sandbox_filter := '((kind(test) & (' + sandbox_package_filter + ')) | (' + sandbox_gateway_filter + '))'
fast_filter := 'not ' + network_filter + ' and not ' + sandbox_filter
sandbox_test_filter := 'not ' + network_filter + ' and ' + sandbox_filter
sandbox_test_threads := '4'

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

# Run the complete local suite with shared prerequisites established once. `--stale` reuses built Wasms.
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source ./script/postgres-up.sh
    # --stale is the sandbox leg's alone; the fast leg forwards its args to nextest.
    fast_args=()
    for arg in "$@"; do [[ "$arg" == --stale ]] || fast_args+=("$arg"); done
    just -- _test-fast "${fast_args[@]}"
    just -- _test-sandbox "$@"

# Run the complete non-node gate.
test-fast *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source ./script/postgres-up.sh
    just -- _test-fast "$@"

_test-fast *args:
    #!/usr/bin/env bash
    set -euo pipefail
    # Include integration targets: this is the complete non-node partition.
    cargo nextest run --ignore-default-filter \
        -E '{{ fast_filter }}' "$@"

# Run the node-backed gate against a pooled neard sandbox. `--stale` reuses the Wasms in target/near.
test-sandbox *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source ./script/postgres-up.sh
    just -- _test-sandbox "$@"

_test-sandbox *args:
    #!/usr/bin/env bash
    set -euo pipefail
    trap './script/sandbox-down.sh || true' EXIT
    nextest_args=()
    sandbox_package_args=()
    sandbox_test_threads='{{ sandbox_test_threads }}'
    use_default_packages=true
    while (($#)); do
        case "$1" in
            -p | -p?* | --package | --package=*)
                use_default_packages=false
                nextest_args+=("$1")
                ;;
            --test-threads)
                shift
                if (($# == 0)); then
                    echo "error: --test-threads requires a value" >&2
                    exit 2
                fi
                sandbox_test_threads="$1"
                ;;
            --test-threads=*)
                sandbox_test_threads="${1#*=}"
                ;;
            --stale)
                export TEST_CONTRACTS_PREBUILT=1
                ;;
            *)
                nextest_args+=("$1")
                ;;
        esac
        shift
    done
    if [[ ! "$sandbox_test_threads" =~ ^[1-9][0-9]*$ ]]; then
        echo "error: sandbox test threads must be a positive integer" >&2
        exit 2
    fi
    if "$use_default_packages"; then
        while IFS= read -r package; do
            sandbox_package_args+=(-p "$package")
        done <<< '{{ sandbox_packages }}'
    fi
    SANDBOX_NODE_COUNT="$sandbox_test_threads" source ./script/sandbox-up.sh
    cargo nextest run --profile sandbox --ignore-default-filter \
        --test-threads "$sandbox_test_threads" \
        "${sandbox_package_args[@]}" \
        -E '{{ sandbox_test_filter }}' "${nextest_args[@]}"

# Start the out-of-band sandbox neard (prints its RPC url). `--stale` skips the Wasm prebuild.
sandbox-up *args:
    #!/usr/bin/env bash
    set -euo pipefail
    for arg in "$@"; do
        case "$arg" in
            --stale)
                export TEST_CONTRACTS_PREBUILT=1
                ;;
            *)
                echo "error: sandbox-up accepts only --stale" >&2
                exit 2
                ;;
        esac
    done
    SANDBOX_NODE_COUNT='{{ sandbox_test_threads }}' ./script/sandbox-up.sh

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

# Warm the shared cache of released contract WASM.
#
# Downloads every pinned release from its GitHub Release and verifies it against
# the SHA-256 in contract/artifacts/src/ids.rs. Migration and upgrade tests
# deploy these bytes, so they need the cache warm (or network access).
#
# The cache lives outside the repository — ~/.cache/templar-contract-artifacts by
# default — so every worktree shares one copy. Override with
# TEMPLAR_ARTIFACT_CACHE; set TEMPLAR_ARTIFACT_OFFLINE=1 to forbid downloads.
#
# Cutting a release is NOT a manual step: merging the release PR tags the
# version and CI builds, uploads, and pins the WASM. See RELEASING.md.
artifacts-fetch:
    cargo run --quiet -p templar-contract-artifacts --features fetch --bin fetch-artifacts
