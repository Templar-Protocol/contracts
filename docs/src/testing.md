# Testing & Code Coverage

## Test Execution

Run the complete local suite through the same entrypoints used by CI:

```bash
just test
```

Use `just test-fast` for the complete non-node gate, including non-node
integration targets, or `just test-sandbox` for the node-backed gate. The sandbox recipe prebuilds NEAR contracts before
starting its pooled `neard` instances.

Run the artifact drift check separately when validating checked-in WASM blobs —
a pure hash/version check with no builds:

```bash
./script/check-artifact-drift.sh
```

## Local Testing

### Running Tests with Coverage

```bash
# Generate and open HTML coverage for the fast library-test cut
just coverage

# Generate coverage.lcov
just coverage-lcov
```

### Test Categories

- **Unit tests**: Module-level functionality
- **Integration tests**: Cross-module interactions
- **Contract tests**: Smart contract behavior
- **End-to-end tests**: Full workflow validation

## Performance Testing

### Gas Usage Analysis

Gas usage analysis is available through existing tools:

```bash
./script/gas-report.sh
```

This generates a gas report for market operations, including average gas costs for individual operations and snapshot iteration limits.
