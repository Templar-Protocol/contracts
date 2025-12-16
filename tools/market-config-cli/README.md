# Market Configuration CLI

A command-line tool for generating and validating Templar market configurations.

## Features

- ✨ **Interactive Mode**: User-friendly prompts guide you through configuration creation
- 📋 **Template Support**: Start from pre-configured templates for common market types
- 🔄 **Copy from Deployed Contracts**: Import configuration from existing contracts
- ✅ **Validation**: Comprehensive validation including:
  - Range checks
  - Decimal precision validation
  - Pyth price feed verification
  - Oracle contract integration
- 📊 **Interest Rate Calculator**: Generate piecewise or linear interest rate curves
- 📦 **Library & CLI**: Use as a CLI tool or integrate as a library

## Installation

### From Source

```bash
cd market-config-cli
cargo build --release
```

The binary will be available at `target/release/market-config-cli`.

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
market-config-cli = { path = "../market-config-cli" }
```

## Usage

### Interactive Mode

The easiest way to create a configuration:

```bash
# Long form
market-config-cli interactive --output my-market-config.json --network testnet

# Shortcut alias
market-config-cli i --output my-market-config.json --network testnet
```

The wizard now keeps the terminal clean: before each step it clears the screen, shows a compact “Current config” overview of fields entered so far, displays progress (e.g., `[3/7] Risk parameters`), then asks the next question. Each field is validated immediately; on validation failure you’re re-prompted on that field instead of continuing with bad data.

### Copy from Deployed Contract

Extract configuration from an existing market:

```bash
market-config-cli from-contract \
  --contract-id default-17092936190.gh-205.templar-in-training.testnet \
  --output extracted-config.json \
  --network testnet
```

After fetching, the CLI can optionally step you through quick edits grouped by section (Basic, Oracle, Risk, Interest Rate, Ranges, Fees, Yield) so you can adjust only what you need.

### Use a Template

Start from a template file:

```bash
market-config-cli from-template \
  --template tests/fixtures/sample_config.json \
  --output new-config.json
```

When loading from a template you can opt into the same sectioned edit flow to tweak values before saving.

### Edit Sections at a Glance

The edit flow splits prompts into concise sections so you only answer what you need, with validation at every field:

- **Basic configuration**: time chunk, assets, protocol account
- **Oracle settings**: oracle account, price IDs, decimals, price age
- **Risk parameters**: collateral ratios, usage ratio, liquidation spread, optional max duration
- **Interest rate strategy**: linear, piecewise, or exponential parameters
- **Ranges**: borrow/supply/withdrawal minimums and optional maximums
- **Fees**: borrow origination and supply withdrawal fee type and values
- **Yield distribution**: supplier weight and static recipients

### Validate Configuration

Validate an existing configuration file:

```bash
market-config-cli validate \
  --config my-market-config.json \
  --network testnet
```

The validator will:

- Check all field constraints
- Verify range consistency
- Validate decimal precision
- Confirm Pyth price feeds exist on the oracle contract

### Calculate Interest Rate Curve

Generate interest rate strategy parameters:

```bash
market-config-cli calculate-curve \
  --starting-rate 0.02 \
  --optimal-rate 0.10 \
  --optimal-usage 0.80 \
  --max-rate 0.50
```

This will output a JSON representation of the interest rate strategy that can be copied into your configuration.

## Configuration Templates

The CLI includes built-in templates for common market types:

- **Conservative Stablecoin**: Low-risk parameters for stablecoin pairs (e.g., USDC/USDT)
- **Standard Crypto**: Typical parameters for volatile crypto collateral (e.g., USDC/NEAR)
- **High Volatility**: Conservative parameters for highly volatile assets

## Library Usage

You can use the CLI as a library in your Rust code:

```rust
use market_config_cli::{ConfigBuilder, ConfigValidator, InterestRateCalculator};
use common::number::Decimal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build a configuration
    let config = ConfigBuilder::new()
        .time_chunk_duration_ms(600_000)
        .borrow_asset("usdc.near")?
        .collateral_asset("wrap.near")?
        .oracle_account_id("pyth-oracle.near")?
        .borrow_price_id([0xbb; 32])
        .borrow_decimals(6)
        .collateral_price_id([0xaa; 32])
        .collateral_decimals(24)
        .price_max_age_s(60)
        .borrow_mcr_maintenance(Decimal::from(125u32) / 100u32)
        .borrow_mcr_liquidation(Decimal::from(120u32) / 100u32)
        // ... more configuration
        .build()?;

    // Validate it
    let validator = ConfigValidator::new(Some("testnet".to_string()));
    validator.validate(&config).await?;

    // Calculate interest rate curve
    let calculator = InterestRateCalculator::new();
    let strategy = calculator.calculate_piecewise(
        "0.02", "0.10", "0.80", "0.50"
    )?;

    Ok(())
}
```

## Configuration Fields

### Basic Configuration

- `time_chunk_duration_ms`: Duration of time chunks for snapshots (milliseconds)
- `borrow_asset`: NEP-141 token contract for the borrowed asset
- `collateral_asset`: NEP-141 token contract for the collateral asset
- `protocol_account_id`: Account to receive protocol fees

### Oracle Configuration

- `oracle_account_id`: Pyth oracle contract account ID
- `borrow_asset_price_id`: Pyth price feed ID (32 bytes hex)
- `borrow_asset_decimals`: Number of decimals for borrow asset
- `collateral_asset_price_id`: Pyth price feed ID (32 bytes hex)
- `collateral_asset_decimals`: Number of decimals for collateral asset
- `price_maximum_age_s`: Maximum acceptable price age in seconds

### Risk Parameters

- `borrow_mcr_maintenance`: Minimum collateralization ratio for healthy positions
- `borrow_mcr_liquidation`: Collateralization ratio threshold for liquidation
- `borrow_asset_maximum_usage_ratio`: Maximum percentage of supply that can be borrowed
- `liquidation_maximum_spread`: Maximum spread for liquidators
- `borrow_maximum_duration_ms`: Optional maximum borrow duration

### Interest Rate

- `borrow_interest_rate_strategy`: Linear or Piecewise curve configuration

### Ranges

- `borrow_range`: Min/max borrow amounts
- `supply_range`: Min/max supply amounts
- `supply_withdrawal_range`: Min/max withdrawal amounts

### Fees

- `borrow_origination_fee`: One-time fee when creating a borrow (flat or percentage)
- `supply_withdrawal_fee`: Time-based fee for early withdrawals

### Yield Distribution

- `yield_weights`: How interest is distributed between suppliers and static recipients

## Validation Rules

The tool enforces several validation rules:

1. **MCR Consistency**: Maintenance MCR must be ≥ liquidation MCR, both must be > 1.0
2. **Usage Ratio**: Must be between 0.0 and 1.0
3. **Interest Rate Cap**: Maximum rate cannot exceed 10,000,000% APY
4. **Range Consistency**: Withdrawal minimum must be ≤ supply minimum
5. **Decimal Limits**: Asset decimals must be ≤ 24 and validated against token on-chain metadata
6. **Asset Uniqueness**: Borrow and collateral assets must be different
7. **Pyth Validation**: Price feed IDs must exist on the oracle contract

## Examples

See the `tests/fixtures/` directory for example configurations.

## Testing

Run the test suite:

```bash
cargo test
```

Run integration tests (requires network access):

```bash
cargo test --test integration_tests -- --ignored
```

## Market config CLI

- Subcommands (aliases): `interactive` (`i`), `from-contract` (`fc`), `from-template` (`ft`), `validate` (`v`), `calculate-curve` (`calc`).
- Interest rate selection uses the concrete `InterestRateStrategy` variants (linear, piecewise, exponential) with dedicated prompts per model.
- Warnings/success messages are styled for clarity; token decimals are validated on-chain (NEP-141 via `ft_metadata`, NEP-245 via bundled omni tokens data).

### CLI walkthroughs

![From template]
![From contract]
![Calculate curve]
