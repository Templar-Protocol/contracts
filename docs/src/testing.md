# Testing and Code Coverage

Templar Protocol maintains comprehensive test suites to ensure protocol security and reliability.

## Test Execution

### Running All Tests
```bash
./script/test.sh
```

This script:
1. Pre-builds all test contracts using `script/prebuild-test-contracts.sh`
2. Runs tests using `cargo nextest run`

### Test Structure

The test suite includes:

- **Unit Tests**: Located in each module's `tests/` directory
- **Integration Tests**: Full protocol interaction tests
- **Market Tests**: Comprehensive market functionality tests
- **Registry Tests**: Contract deployment and management tests
- **Oracle Tests**: Limited LST oracle and price transformation tests

### Test Categories

#### Market Contract Tests
- **Basic Operations**: Supply, borrow, collateralize, withdraw
- **Liquidation**: Liquidation scenarios and edge cases
- **Interest Rates**: Interest accrual and rate calculations
- **MCR (Minimum Collateralization Ratio)**: Maintenance and liquidation ratios
- **Edge Cases**: Boundary conditions and error states

#### Registry Contract Tests
- **Deployment**: Market deployment from registry
- **Version Management**: Adding and managing contract versions
- **Access Control**: Admin permissions and restrictions

#### Integration Tests
- **End-to-End Workflows**: Complete user journeys
- **Cross-Contract Interactions**: Registry-to-market deployments
- **Oracle Integration**: Basic LST oracle and price transformation tests

## Test Coverage

### Current Coverage Areas

The test suite covers:

1. **Core Protocol Logic**
   - Asset management operations (supply, borrow, collateralize, withdraw)
   - Interest rate calculations and accumulation
   - Liquidation mechanics and edge cases
   - Collateralization requirements and validation

2. **Smart Contract Interfaces**
   - NEP-141 (Fungible Token) integration
   - NEP-245 (Multi Token) support  
   - Basic oracle price feed integration
   - Cross-contract interactions

3. **Error Handling**
   - Invalid parameter handling
   - Access control violations  
   - Arithmetic overflow/underflow protection
   - Edge case boundary testing

4. **Integration Testing**
   - Full protocol workflows
   - Registry-to-market deployments
   - Basic oracle integration (LST oracle and price transformation)

### Test Execution Statistics

```
Test Results (Current):
├── Total Tests Available: 280
├── Includes unit tests for all contracts and modules
├── Integration tests for full protocol workflows
├── Bot tests for liquidation and accumulator logic
└── Utility tests for common functionality

Success Rate: 100% (all tests passing)
```

**Note**: Specific code coverage percentages require additional tooling setup.

### Continuous Integration

Tests run automatically on:
- Production deployment builds
- Staging deployment builds (triggered by pull requests)

**Note**: Tests are executed via `workflow_call` in deployment workflows, not directly on pull requests or commits.

## Test Environment

Tests use:
- **near-workspaces**: NEAR blockchain simulation
- **Mock Contracts**: Isolated testing environment
- **Deterministic Scenarios**: Reproducible test conditions

### Mock Components
- **Mock FT**: Simulated fungible tokens
- **Mock Oracle**: Controlled price feeds
- **Mock MT**: Multi-token simulation

## Coverage Reporting

### Available Tools

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

This generates a comprehensive gas report for all market operations, including:
- Individual function gas costs
- Snapshot iteration limits  
- Performance optimization targets

## Test Data

Test scenarios include:
- **Market Configuration Testing**: Using test configurations with sample assets
- **Basic Functionality Testing**: Standard protocol operations
- **Edge Case Testing**: Boundary conditions and error states

## Quality Assurance

- **Automated Testing**: Tests run in deployment workflows
- **Code Review**: GitHub pull request review process
- **Security Testing**: Basic security-related tests included in test suite
- **Regression Testing**: Full test suite runs with each deployment
