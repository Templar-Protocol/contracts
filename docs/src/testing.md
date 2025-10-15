# Testing

Templar Protocol maintains comprehensive test suites to ensure protocol security and reliability.

## Test Execution

Inoke the test suite with this script:

```bash
./script/test.sh
```

## Continuous Integration

Tests run automatically on:
- Production deployment builds
- Staging deployment builds (triggered by pull requests)

**Note**: Tests are executed via `workflow_call` in deployment workflows, not directly on pull requests or commits.

## Coverage Reporting

To generate coverage reports, additional tooling can be installed:

```bash
# Install cargo-llvm-cov (optional)
cargo install cargo-llvm-cov

# Generate coverage report (when tooling is set up)
cargo llvm-cov --html --output-dir coverage-report

# View report
open coverage-report/index.html
```

**Note**: Coverage reporting tools are not integrated into the CI pipeline.

## Performance Testing

### Gas Usage Analysis
Gas usage analysis is available through existing tools:

```bash
./script/ci/gas-report.sh
```

This generates a gas report for market operations, including average gas costs for individual operations and snapshot iteration limits.
