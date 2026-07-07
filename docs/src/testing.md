# Testing & Code Coverage

## Test Execution

Invoke the test suite with this script:

```bash
./script/test.sh
```

The script prebuilds NEAR contracts with the fast non-reproducible test profile
before running the test suite.

Run the reproducible artifact drift check separately when validating checked-in
WASM blobs:

```bash
./script/check-artifact-drift.sh
```

## Local Testing

### Running Tests with Coverage
```bash
# Install coverage tool
cargo install cargo-llvm-cov

# Prepare test contracts quickly
./script/prebuild-test-contracts.sh --profile test

# Generate coverage report
cargo llvm-cov --html --output-dir coverage-report

# Generate coverage report (ignore test failures)
cargo llvm-cov --html --output-dir coverage-report --ignore-run-fail

# View HTML report
open coverage-report/html/index.html
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
