# Testing & Code Coverage

## Test Coverage Analysis

Current code coverage metrics for the Templar Protocol:

### Overall Coverage
- **Function Coverage**: 43.05% (437/1015 functions)
- **Line Coverage**: 42.38% (3,639/8,586 lines)
- **Region Coverage**: 41.09% (1,377/3,351 regions)
- **Test Results**: 279 tests passing

### Module-Specific Coverage

#### Core Libraries (common/)
- **Number utilities**: 82.86% function, 89.60% line coverage
- **Price handling**: 93.33% function, 93.24% line coverage
- **Interest rate strategy**: 85.00% function, 87.50% line coverage
- **Asset management**: 78.38% function, 67.70% line coverage
- **Withdrawal queue**: 39.39% function, 69.23% line coverage
- **Chunked append-only list**: 78.26% function, 93.06% line coverage

#### Test Utilities (test-utils/)
- **FT controller**: 100% coverage across all metrics
- **Oracle controller**: 100% coverage across all metrics
- **Market controller**: 87.80% function, 83.64% line coverage
- **Storage management**: 100% coverage across all metrics

#### Bot Components (bots/)
- **Liquidator**: 53.33% function, 88.14% line coverage
- **Accumulator**: 0% coverage (not tested in current suite)
- **Swap logic**: 0% coverage (not tested in current suite)

#### Service Layer (service/relayer/)
- **App module**: 89.47% function, 71.03% line coverage
- **Cache**: 76.00% function, 69.66% line coverage
- **Database client**: 78.26% function, 62.95% line coverage
- **NEAR client**: 44.74% function, 39.69% line coverage

## Test Execution

Invoke the test suite with this script:

```bash
./script/test.sh
```

## Local Testing

### Running Tests with Coverage
```bash
# Install coverage tool
cargo install cargo-llvm-cov

# Prepare test contracts
./script/prebuild-test-contracts.sh

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
./script/ci/gas-report.sh
```

This generates a gas report for market operations, including average gas costs for individual operations and snapshot iteration limits.
