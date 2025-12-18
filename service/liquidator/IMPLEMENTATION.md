# Implementation Guide

Technical architecture for developers building on or contributing to the liquidator.

## Architecture

```
┌─────────────────────────────────────────────────┐
│              LiquidatorService                  │
│  - Registry management                          │
│  - Market discovery                             │
│  - Scheduling                                   │
└──────────────────┬────────────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────────┐
│               Liquidator (per market)           │
│  ┌───────────────────────────────────────────┐  │
│  │ Scanner: Find liquidatable positions      │  │
│  └───────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────┐  │
│  │ Strategy: Calculate liquidation amount    │  │
│  └───────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────┐  │
│  │ Executor: Submit transactions + swaps     │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────────┐
│           InventoryManager                      │
│  - Track available balances                     │
└─────────────────────────────────────────────────┘
```

## Core Components

### 1. InventoryManager

Tracks available assets for liquidation.

**Location:** `src/inventory.rs`

```rust
pub struct InventoryManager {
    client: JsonRpcClient,
    account_id: AccountId,
    balances: HashMap<FungibleAsset<BorrowAsset>, U128>,
    collateral_balances: HashMap<FungibleAsset<CollateralAsset>, U128>,
}
```

**Key methods:**
- `refresh()` - Update borrow asset balances
- `refresh_collateral()` - Update collateral asset balances
- `get_available_balance()` - Query balance for asset

### 2. LiquidationStrategy

Determines liquidation amount.

**Location:** `src/liquidation_strategy.rs`

```rust
pub trait LiquidationStrategy {
    fn calculate_liquidation_amount(
        &self,
        position: &BorrowPosition,
        available_balance: U128,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
    ) -> Result<Option<U128>>;
}
```

**Implementations:**
- `PartialLiquidationStrategy` - Use percentage of available funds per liquidation
- `FullLiquidationStrategy` - Use 100% of available funds up to liquidatable amount
- `FixedAmountLiquidationStrategy` - Use fixed amount per liquidation (ideal for loop liquidation)

### 3. CollateralStrategy

Immediate post-liquidation swap behavior.

**Location:** `src/liquidator.rs`, `src/executor.rs`

```rust
pub enum CollateralStrategy {
    Hold,
    SwapToBorrow,
}
```

**SwapToBorrow routing:**
- After each successful liquidation, immediately swaps received collateral back to borrow asset
- Uses configured swap provider (OneClick or Ref Finance)
- Executes in executor module as part of liquidation transaction flow

### 4. Liquidator

Main execution coordinator.

**Location:** `src/liquidator.rs`

```rust
pub struct Liquidator {
    scanner: MarketScanner,
    oracle_fetcher: OracleFetcher,
    executor: LiquidationExecutor,
    market: AccountId,
    market_config: MarketConfiguration,
    strategy: Arc<dyn LiquidationStrategy>,
}
```

**Flow:**
1. Scanner finds liquidatable positions
2. Check available inventory
3. Strategy calculates amount (capped by inventory)
4. Profitability check
5. Executor submits transaction
6. Immediate swap if CollateralStrategy::SwapToBorrow
7. Loop liquidation: if enabled and position still liquidatable, repeat from step 2

### 5. SwapProvider

Collateral swap execution.

**Location:** `src/swap/`

**Trait:**
```rust
#[async_trait]
pub trait SwapProvider {
    async fn swap(&self, params: SwapParams) -> Result<SwapResult>;
}
```

**Implementations:**
- `OneClickSwap` - NEAR Intents API
- `RefSwap` - Ref/Rhea Finance DEX

## Configuration

### Environment Variables

**Core:**
```bash
SIGNER_ACCOUNT_ID=liquidator.near
SIGNER_KEY=ed25519:...
REGISTRY_ACCOUNT_IDS=v1.tmplr.near
NETWORK=mainnet
```

**Liquidation:**
```bash
LIQUIDATION_STRATEGY=partial  # partial | full | fixed-amount
PARTIAL_LIQUIDATION_PERCENTAGE=50         # % of available funds to use
FIXED_LIQUIDATION_AMOUNT_USD=100  # USD amount (for fixed-amount, works across all USD markets)
LOOP_LIQUIDATION=false        # Repeatedly liquidate until healthy
MAX_LOOP_ITERATIONS=10        # Safety limit for loop liquidation
MIN_PROFIT_BPS=50
```

**Collateral:**
```bash
COLLATERAL_STRATEGY=swap-to-borrow  # hold | swap-to-borrow
ONECLICK_API_TOKEN=...              # Optional 1-Click auth
REF_CONTRACT=v2.ref-finance.near    # Mainnet default
```

**Intervals:**
```bash
LIQUIDATION_SCAN_INTERVAL=600   # Seconds
REGISTRY_REFRESH_INTERVAL=3600  # Seconds
```

**Market Filtering:**
```bash
ALLOWED_COLLATERAL_ASSETS=nep141:btc.omft.near,nep141:wrap.near
IGNORED_COLLATERAL_ASSETS=nep141:meta-pool.near
```

## Execution Flow

```
1. Service Loop
   ├─ Refresh registries (every REGISTRY_REFRESH_INTERVAL)
   ├─ Refresh inventory
   ├─ Run liquidation round
   │  ├─ Scan markets
   │  └─ Execute liquidations (with immediate swaps if configured)
   └─ Sleep LIQUIDATION_SCAN_INTERVAL

2. Liquidation Round
   For each market:
     For each position:
       ├─ Check if liquidatable
       ├─ Check inventory balance
       ├─ Calculate amount (strategy: partial or full)
       ├─ Submit liquidation transaction
       ├─ Immediate swap if swap-to-borrow enabled
       └─ If loop_liquidation enabled and position still unhealthy, repeat
```

## Adding New Features

### New Liquidation Strategy

1. Implement `LiquidationStrategy` trait
2. Add to `src/liquidation_strategy.rs`
3. Update `Args::build_config()` to parse new strategy name

### New Collateral Strategy

1. Add variant to `CollateralStrategy` enum
2. Implement handling logic in `executor::execute_liquidation()`
3. Update config parsing in `Args::build_config()`

### New Swap Provider

1. Implement `SwapProvider` trait
2. Add module to `src/swap/`
3. Update `SwapProviderImpl` enum
4. Add configuration parameters to `Args`

### Market Scanner Extension

Modify `src/scanner.rs`:
- `scan()` - Core scanning logic
- `fetch_positions()` - Position retrieval
- `identify_liquidatable()` - Health check logic

## Error Handling

**Retry:** RPC rate limits, network timeouts
**Skip:** Insufficient inventory, position healthy
**Fatal:** Invalid credentials, critical RPC failures

## Testing

```bash
cargo test              # All unit tests
cargo llvm-cov --html   # Coverage report
```

Tests are colocated with implementation in `#[cfg(test)]` modules:
- `src/inventory.rs` - Inventory management
- `src/profitability.rs` - Profit calculations
- `src/liquidation_strategy.rs` - Strategy logic
- `src/config.rs` - Configuration parsing
- `src/rpc.rs` - RPC client

## Swap Providers

### OneClick (NEAR Intents)

**Flow:**
1. `POST /v0/quote` - Get deposit address
2. Create implicit account + register storage
3. Transfer tokens to deposit address
4. `POST /v0/deposit/submit` - Initiate swap
5. Poll `GET /v0/status` until SUCCESS

**Config:**
```bash
ONECLICK_API_TOKEN=...  # Optional (0% fee)
```

### Ref Finance

**Flow:**
1. Register storage (if needed)
2. Call `ft_transfer_call` to REF contract
3. Parse swap result from callback

**Config:**
```bash
REF_CONTRACT=v2.ref-finance.near  # Mainnet
```

## Profitability Calculation

```rust
expected_collateral_value = liquidated_amount * liquidation_bonus
profit = expected_collateral_value - liquidated_amount - gas_cost
min_profit = liquidated_amount * (min_profit_bps / 10000)

profitable = profit >= min_profit
```

## Key Files

- `src/main.rs` - Entry point
- `src/config.rs` - Configuration parsing
- `src/service.rs` - Main orchestrator
- `src/liquidator.rs` - Liquidation coordinator
- `src/scanner.rs` - Market scanning
- `src/executor.rs` - Transaction execution + immediate swaps
- `src/inventory.rs` - Inventory management
- `src/liquidation_strategy.rs` - Liquidation strategies
- `src/swap/` - Swap provider implementations

## Monitoring

**Logs:** `RUST_LOG=info,templar_liquidator=debug`

**Key metrics to track:**
- Liquidations per hour
- Success rate
- Average profit per liquidation
- Inventory turnover
- Swap success rate

## Docker

See [README.md](./README.md) for Docker commands.

**Build context:** `contracts/` (repository root)
**Dockerfile location:** `service/liquidator/Dockerfile`
