# Templar contract-mvp — project tasks. Run `just` to list recipes.

set positional-arguments

# `(^|::)` so the convention reaches tests nested in modules, not only those at
# an integration binary's root. A bare `^` anchors against the full path
# (`module::tests::requires_network_x`), so nested network tests silently ran in
# the fast gate.
network_filter := 'test(/(^|::)requires_network_/)'
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
artifacts_bin := 'cargo run --quiet -p templar-contract-artifacts --features fetch,clap --bin fetch-artifacts --'

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
        # Build test binaries BEFORE starting the pool: nodes produce blocks from
        # boot, so starting them first spends the whole cargo build competing for
        # CPU (minutes of contention on CI). Default set only — a narrowed `-p`
        # run keeps its package flags in nextest_args, which cargo build can't take.
        cargo build --tests "${sandbox_package_args[@]}"
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

# Benchmark the sandbox harness primitives on a dedicated neard (never a pooled
# node) — block-latency floor, per-tx and per-patch costs, fixture setup.
bench-sandbox *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source ./script/prebuild-test-contracts.sh
    # Debug profile, like the test gate itself: the numbers must be comparable
    # with what tests actually pay, and the work being timed is node I/O.
    cargo run -p templar-gateway-testing --bin sandbox-bench -- "$@"

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
# Downloads every release pinned in contract/artifacts/releases/ and verifies
# its SHA-256; migration and upgrade tests deploy these bytes. The cache is
# outside the repo (override with TEMPLAR_ARTIFACT_CACHE;
# TEMPLAR_ARTIFACT_OFFLINE=1 forbids downloads).
artifacts-fetch:
    {{ artifacts_bin }}

# Print the resolved artifact cache directory.
artifacts-cache-path:
    @{{ artifacts_bin }} --print-path

# Delete the artifact cache. Entries are immutable, so this only costs the next
# `just artifacts-fetch` a re-download.
artifacts-clean:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="$({{ artifacts_bin }} --print-path)"
    # The crate resolves the path, so this cannot delete a directory it does not
    # manage — but an empty expansion would make the `rm` unbounded.
    [ -n "$dir" ] || { echo "could not resolve the artifact cache directory" >&2; exit 1; }
    rm -rf -- "$dir"
    echo "deleted $dir"

# Report whether a release tag's WASM is built, published and recorded.
release-wasm-status +tags:
    #!/usr/bin/env bash
    set -euo pipefail
    for tag in "$@"; do
        if cut -f3 contract/artifacts/releases/*.tsv | grep -Fxq "$tag"; then
            recorded=yes
        else
            recorded=no
        fi
        if gh release view "$tag" --json assets \
             --jq '.assets[].name | select(endswith(".wasm"))' 2>/dev/null |
             grep -q .; then
            published=yes
        else
            published=no
        fi
        case "${recorded}/${published}" in
            yes/yes) echo "${tag}: built and recorded" ;;
            no/yes)  echo "${tag}: published, not yet recorded — the catalog PR is pending" ;;
            yes/no)  echo "${tag}: recorded, but the release carries no WASM asset" ;;
            no/no)   echo "${tag}: not built" ;;
        esac
    done

# Build and publish a released version's canonical WASM, and queue its catalog row.
release-wasm +tags:
    #!/usr/bin/env bash
    set -euo pipefail
    for tag in "$@"; do
        git rev-parse -q --verify "refs/tags/${tag}" >/dev/null ||
            { echo "no such tag: ${tag} (try 'git fetch --tags')" >&2; exit 1; }

        if cut -f3 contract/artifacts/releases/*.tsv | grep -Fxq "$tag"; then
            echo "${tag}: already recorded; nothing to build"
            continue
        fi

        # Exit code only. The resolved *version* comes from the local worktree's
        # Cargo.toml, not the tag, so it is meaningless outside a checkout of it.
        status=0
        err=$(cargo run --quiet -p templar-contract-artifacts \
            --features workspace-loader,clap --bin prebuild-test-contracts -- \
            --resolve "$tag" 2>&1 >/dev/null) || status=$?
        if [ "$status" -eq 2 ]; then
            echo "${tag}: names no catalogued NEAR WASM artifact" >&2
            exit 1
        elif [ "$status" -ne 0 ]; then
            printf '%s\n' "$err" >&2
            exit "$status"
        fi

        # Dispatched against the default branch, never the tag: GitHub runs the
        # workflow file as it exists at the ref it is dispatched against.
        gh workflow run release-artifacts.yml --ref dev -f tag="$tag"
        echo "${tag}: dispatched"
    done
