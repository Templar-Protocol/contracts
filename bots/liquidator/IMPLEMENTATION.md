# Implementation Guide

Technical architecture for developers building on or contributing to the liquidator.

## Architecture

```
┌─────────────────────────────────────────────────┐
│              LiquidatorService                  │
│  - Registry management                          │
│  - Market discovery                             │
│  - Scheduling                                   │
└──────────────────┬──────────────────────────────┘
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
│  │ Executor: Submit transactions             │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────────┐
│           InventoryManager                      │
│  - Track available balances                     │
│  - Record liquidation history                   │
└─────────────────────────────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────────┐
│          InventoryRebalancer                    │
│  - Apply collateral strategy                    │
│  - Execute swaps via providers                  │
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
- `record_liquidation()` - Track collateral→borrow mapping

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
- `PartialLiquidationStrategy` - Liquidate percentage of liquidatable amount
- `FullLiquidationStrategy` - Liquidate 100% of liquidatable amount (from contract)

### 3. CollateralStrategy

Post-liquidation rebalancing.

**Location:** `src/collateral_strategy.rs`

```rust
pub enum CollateralStrategy {
    Hold,
    SwapToPrimary { primary_asset: FungibleAsset<BorrowAsset> },
    SwapToBorrow,
}
```

**SwapToBorrow routing:**
1. Check liquidation history for collateral asset
2. Find markets that use this collateral
3. Route to highest balance borrow asset

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
LIQUIDATION_STRATEGY=partial  # partial | full
PARTIAL_PERCENTAGE=50
MIN_PROFIT_BPS=50
MAX_GAS_PERCENTAGE=10
```

**Collateral:**
```bash
COLLATERAL_STRATEGY=swap-to-borrow  # hold | swap-to-primary | swap-to-borrow
PRIMARY_ASSET=nep141:usdc.near      # For swap-to-primary
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
   │  ├─ Execute liquidations
   │  └─ Refresh collateral inventory
   ├─ Rebalance inventory (apply collateral strategy)
   └─ Sleep LIQUIDATION_SCAN_INTERVAL

2. Liquidation Round
   For each market:
     ├─ Fetch liquidatable positions
     ├─ Check inventory balance
     ├─ Calculate amount (min: position, inventory, profitable amount)
     ├─ Submit transaction
     └─ Record collateral received

3. Inventory Rebalancing
   For each collateral holding:
     ├─ Apply collateral strategy
     ├─ If swap needed: query provider, execute swap
     └─ Update inventory
```

## Adding New Features

### New Liquidation Strategy

1. Implement `LiquidationStrategy` trait
2. Add to `src/liquidation_strategy.rs`
3. Update `Args::build_config()` to parse new strategy name

### New Collateral Strategy

1. Add variant to `CollateralStrategy` enum
2. Implement routing logic in `InventoryRebalancer::rebalance()`
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

profitable = profit >= min_profit && gas_cost <= max_gas_percentage * liquidated_amount
```

## Key Files

- `src/main.rs` - Entry point
- `src/config.rs` - Configuration parsing
- `src/service.rs` - Main orchestrator
- `src/liquidator.rs` - Liquidation coordinator
- `src/scanner.rs` - Market scanning
- `src/executor.rs` - Transaction execution
- `src/inventory.rs` - Inventory management
- `src/rebalancer.rs` - Collateral rebalancing
- `src/liquidation_strategy.rs` - Liquidation strategies
- `src/collateral_strategy.rs` - Collateral strategies
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
**Dockerfile location:** `bots/liquidator/Dockerfile`
