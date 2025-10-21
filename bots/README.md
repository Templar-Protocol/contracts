# Templar Bots

Production-grade liquidation and accumulation bots for Templar Protocol on NEAR blockchain.

## Architecture

The bots are organized as a Cargo workspace with three crates:

```
bots/
├── common/          # Shared RPC utilities and types
├── accumulator/     # Price accumulation bot
└── liquidator/      # Liquidation bot (main focus)
```

## Liquidator Bot

Monitors Templar lending markets and performs liquidations when borrowers fall below their collateralization ratio.

### Key Features

- **Strategy Pattern**: Pluggable liquidation strategies (Partial/Full)
- **Multiple Swap Providers**: RheaSwap and NEAR Intents integration
- **Production-Ready**: Comprehensive error handling, logging, and profitability analysis
- **Gas Optimization**: Smart profitability checks prevent unprofitable liquidations
- **Concurrent Processing**: Configurable concurrency for high throughput

### Components

- `liquidator/src/lib.rs` - Core liquidation logic with Liquidator struct
- `liquidator/src/main.rs` - Executable service that runs in a loop
- `liquidator/src/strategy.rs` - Liquidation strategies (Partial/Full)
- `liquidator/src/swap/` - Swap provider implementations
  - `mod.rs` - SwapProvider trait and wrapper
  - `rhea.rs` - Rhea Finance DEX integration
  - `intents.rs` - NEAR Intents cross-chain swap integration
- `common/src/lib.rs` - Shared RPC utilities (view, send_tx, etc.)

### Prerequisites

- Rust (install via rustup)
- NEAR account with sufficient balance
- NEAR CLI (for deploying and interacting with contracts)
- Deployed NEAR contracts for the lending protocol
- Oracle contract for price data

### Running the Bot

```bash
liquidator \
    --registries registry1.testnet registry2.testnet \
    --signer-key ed25519:<YOUR_PRIVATE_KEY_HERE> \
    --signer-account liquidator.testnet \
    --asset nep141:usdc.testnet \
    --swap near-intents \
    --network testnet \
    --timeout 60 \
    --interval 600 \
    --registry-refresh-interval 3600 \
    --concurrency 10 \
    --partial-percentage 50 \
    --min-profit-bps 50 \
    --max-gas-percentage 10
```

### CLI Arguments

#### Required Arguments

- `--registries` - List of registry contracts to query for markets (e.g., `templar-registry1.testnet`)
- `--signer-key` - Private key of the signer account (format: `ed25519:...`)
- `--signer-account` - NEAR account that will perform liquidations (e.g., `liquidator.testnet`)
- `--asset` - Asset specification for liquidations, format: `nep141:<token>` or `nep245:<contract>/<token_id>`
- `--swap` - Swap provider to use: `rhea-swap` or `near-intents`

#### Optional Arguments

- `--network` - NEAR network to connect to: `testnet` or `mainnet` (default: `testnet`)
- `--timeout` - Timeout for RPC calls in seconds (default: `60`)
- `--interval` - Interval between liquidation runs in seconds (default: `600`)
- `--registry-refresh-interval` - Interval to refresh market list in seconds (default: `3600`)
- `--concurrency` - Number of concurrent liquidation attempts (default: `10`)
- `--partial-percentage` - Percentage of position to liquidate (1-100, default: `50`)
- `--min-profit-bps` - Minimum profit margin in basis points (default: `50` = 0.5%)
- `--max-gas-percentage` - Maximum gas cost as percentage of liquidation amount (default: `10`)

### How It Works

1. **Market Discovery**: Fetches all deployed markets from specified registries
2. **Position Monitoring**: Continuously checks borrower positions in each market
3. **Oracle Prices**: Fetches current prices from oracle contract
4. **Liquidation Decision**:
   - Checks if borrower is below required collateralization ratio
   - Calculates liquidation amount using configured strategy
   - Validates profitability (considering gas costs and profit margin)
5. **Swap Execution**: If needed, swaps assets to obtain borrow asset
6. **Liquidation**: Sends `ft_transfer_call` to trigger liquidation
7. **Logging**: Records all attempts with success/failure details

### Liquidation Strategies

#### Partial Liquidation (Default)

Liquidates a percentage of the position (default: 50%):
- Reduces market impact
- Lower gas costs (~40-60% savings)
- Allows multiple liquidators to participate
- More gradual approach to underwater positions

#### Full Liquidation

Liquidates the entire position:
- Maximizes immediate profit
- Clears position completely
- Higher gas costs
- More aggressive approach

### Swap Providers

#### Rhea Finance

Production-ready DEX integration:
- Concentrated liquidity pools (DCL)
- Configurable fee tiers (default: 0.2%)
- NEP-141 token support
- Contract: `dclv2.ref-finance.near` (mainnet), `dclv2.ref-dev.testnet` (testnet)

#### NEAR Intents

Cross-chain swap integration:
- Solver network for best execution
- 120+ assets across 20+ chains
- NEP-141 and NEP-245 support
- HTTP JSON-RPC to Defuse Protocol solver relay
- Contract: `intents.near` (mainnet), `intents.testnet` (testnet)

### Code Examples

#### Fetching Market Configuration

```rust
async fn get_configuration(&self) -> LiquidatorResult<MarketConfiguration> {
    view(
        &self.client,
        self.market.clone(),
        "get_configuration",
        json!({}),
    )
    .await
    .map_err(LiquidatorError::GetConfigurationError)
}
```

#### Fetching Oracle Prices

```rust
async fn get_oracle_prices(
    &self,
    oracle: AccountId,
    price_ids: &[PriceIdentifier],
    age: u32,
) -> LiquidatorResult<OracleResponse> {
    view(
        &self.client,
        oracle,
        "list_ema_prices_no_older_than",
        json!({ "price_ids": price_ids, "age": age }),
    )
    .await
    .map_err(LiquidatorError::PriceFetchError)
}
```

#### Calculating Liquidation Amount

The strategy determines how much to liquidate based on:
1. Market's maximum liquidatable amount
2. Strategy percentage (for partial liquidations)
3. Available balance in bot's wallet
4. Economic viability (minimum 10% of full amount)
5. Profitability after gas costs

```rust
// From strategy.rs
let liquidation_amount = strategy.calculate_liquidation_amount(
    position,
    oracle_response,
    configuration,
    available_balance,
)?;
```

#### Profitability Check

```rust
// From strategy.rs
fn should_liquidate(
    &self,
    swap_input_amount: U128,
    liquidation_amount: U128,
    expected_collateral: U128,
    gas_cost_estimate: U128,
) -> LiquidatorResult<bool> {
    // Calculate total cost
    let total_cost = swap_input_amount.0 + gas_cost_estimate.0;

    // Add profit margin (e.g., 50 bps = 0.5%)
    let profit_margin_multiplier = 10_000 + self.min_profit_margin_bps as u128;
    let min_revenue = (total_cost * profit_margin_multiplier) / 10_000;

    // Check if collateral covers cost + margin
    Ok(expected_collateral.0 >= min_revenue)
}
```

#### Creating Liquidation Transaction

```rust
fn create_transfer_tx(
    &self,
    borrow: &AccountId,
    liquidation_amount: U128,
    nonce: u64,
    block_hash: CryptoHash,
) -> LiquidatorResult<Transaction> {
    let msg = serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
        account_id: borrow.clone(),
        amount: None,
    }))?;

    Ok(Transaction::V0(TransactionV0 {
        nonce,
        receiver_id: self.asset.contract_id().clone(),
        block_hash,
        signer_id: self.signer.account_id.clone(),
        public_key: self.signer.public_key().clone(),
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer_call".to_string(),
            args: serialize_and_encode(json!({
                "receiver_id": self.market,
                "amount": liquidation_amount,
                "msg": msg,
            })),
            gas: DEFAULT_GAS,
            deposit: NearToken::from_yoctonear(1).as_yoctonear(),
        }))],
    }))
}
```

### Deployment Model

**Single Bot Instance per Organization**:
- One liquidator instance monitors multiple registries
- Each registry contains multiple markets
- Balance is shared across all markets
- Real-time balance queries via `ft_balance_of`

Example topology:
```
Single Liquidator Bot
  ├─> Registry 1 (10 markets)
  ├─> Registry 2 (15 markets)
  └─> Registry 3 (8 markets)
Total: 33 markets monitored
```

### Balance Management

The bot queries on-chain balance in real-time:

```rust
// From lib.rs
async fn get_asset_balance<A: AssetClass>(
    &self,
    asset: &FungibleAsset<A>,
) -> LiquidatorResult<U128> {
    let balance_action = asset.balance_of_action(&self.signer.get_account_id());

    let balance = view::<U128>(
        &self.client,
        asset.contract_id().into(),
        &balance_action.method_name,
        args,
    ).await?;

    Ok(balance)
}
```

**Funding the Bot**:
1. Transfer borrow assets to bot account (e.g., USDC)
2. Bot automatically checks balance before each liquidation
3. Receives collateral after successful liquidations
4. Manually swap collateral back to borrow asset as needed

### Testing

```bash
# Run all tests
cargo test -p templar-liquidator

# Run with coverage
cargo llvm-cov --package templar-liquidator --lib --tests

# Run specific test
cargo test -p templar-liquidator --lib test_partial_liquidation_strategy
```

Current test coverage: 37% (68 tests, all passing)
- Strategy module: 99.32% coverage
- Appropriate for network-heavy bot

### Building

```bash
# Build all workspace members
cargo build -p templar-bots-common -p templar-accumulator -p templar-liquidator --bins

# Build release
cargo build --release -p templar-liquidator --bin liquidator
```

### Monitoring

The bot uses structured logging via `tracing`:
- Set `RUST_LOG=info` for normal operation
- Set `RUST_LOG=debug` for detailed RPC calls
- Set `RUST_LOG=trace` for full debugging

Example logs:
```
INFO liquidator: Running liquidations for market: market1.testnet
DEBUG liquidator: Fetching borrow positions
INFO liquidator: Found 5 positions to check
INFO liquidator: Position user.testnet is liquidatable
DEBUG liquidator: Calculated liquidation amount: 1000 USDC
INFO liquidator: Liquidation successful, received 0.05 BTC collateral
```

### Error Handling

Comprehensive error types in `LiquidatorError`:
- RPC errors (network, timeouts)
- Price oracle errors
- Swap provider errors
- Insufficient balance errors
- Strategy validation errors

Failed liquidations are logged but don't stop the bot - it continues processing other positions.

### Security Considerations

- **Slippage Protection**: Configurable maximum slippage on swaps
- **Gas Cost Limits**: Prevents unprofitable liquidations
- **Balance Checks**: Validates sufficient funds before operations
- **Timeout Handling**: Prevents stuck transactions
- **Private Key Security**: Use environment variables, never commit keys

### Performance

- **Concurrency**: Default 10 concurrent liquidations
- **Batching**: Fetches 100 positions per page, 500 markets per registry
- **Partial Liquidations**: ~40-60% gas savings vs full liquidations
- **Early Exit**: Profitability checks before expensive swap operations

## Accumulator Bot

(Future documentation - currently basic implementation)

## Common Utilities

The `common` crate provides shared functionality:

- `view()` - Query view methods on contracts
- `send_tx()` - Send signed transactions with retry logic
- `get_access_key_data()` - Fetch nonce and block hash for transactions
- `list_deployments()` - Paginated fetching of market deployments
- `Network` enum - Mainnet/testnet configuration

## License

MIT License - Same as Templar Protocol
