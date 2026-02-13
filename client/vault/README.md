# Templar Vault Client SDK

gm ser - welcome to the Vault SDK.

## Why Not WASM?

We initially explored a WASM-based approach for the any-language SDK. While functional, we abandoned it for several reasons:

1. **Signing Architecture Mismatch** - The vault operations rely heavily on internal Rust client machinery for transaction signing. WASM requires delegating this back to the frontend, creating unnecessary complexity.

2. **Patching Overhead** - Getting WASM to work required extensive patching of existing Rust circuitry, making maintenance a burden.

3. **Single-Threaded Performance** - WASM runs single-threaded, meaning cryptographic operations (signing, hashing) become a bottleneck.

There *is* a way to keep WASM and delegate intent plans for signing by the frontend, but this is overengineered for the actual use case. From an SDK perspective, curator/allocator bots perform a focused set of operations:

- `storage_deposit`
- `deposit`
- `refresh_markets`
- `withdraw` / `redeem`
- `allocate`

Rather than exposing the full complexity of the vault contract, we lock-in these flows with proper gas attachment, nonce handling, and retry logic. The approach taken here is:

1. **Generate contract ABI** - Extract the full ABI from the vault contract
2. **Provide a generator library** - Macros that generate type-safe method bindings
3. **Emit prepared flows** - Curated, production-ready transaction builders

Frontend users interact with the vault directly through Wallet Selector using the generated ABI types - this SDK is purpose-built for backend automation.

---

## FFI Capabilities

The SDK uses **UniFFI** to generate native bindings for multiple languages from a single Rust codebase.

### Supported Platforms

| Platform | Binding Type | Status |
|----------|--------------|--------|
| **Python** | Native async/await | Production |
| **TypeScript** | Generated types from ABI | Production |
| **Rust** | Direct library usage | Production |

### How It Works

UniFFI scaffolding exports all public types and async methods:

```rust
#[uniffi::export(async_runtime = "tokio")]
impl VaultClient {
    pub async fn get_total_assets(&self) -> Result<ForeignU128, ErrorWrapper> { ... }
    pub async fn deposit(&self, amount: ForeignU128, ...) -> Result<..., ErrorWrapper> { ... }
}
```

Custom type mappings handle NEAR-specific types:

```rust
pub struct AccountId(String);      // NEAR account ID
pub struct MarketId(pub u32);      // Market identifier
pub struct CapGroupId(pub String); // Capital group ID
```

### Build Targets

```bash
make python              # Build Python bindings (debug)
make python MODE=release # Build Python bindings (release)
make gen                 # Generate UniFFI scaffolding only
make abi                 # Generate contract ABI + TypeScript types
make smoke-test          # Run Python integration tests
```

---

## Architecture

### Multi-Key Pool with Least-Loaded Selection

The `VaultClient` manages a pool of NEAR access keys for high-concurrency operations:

```
┌─────────────────────────────────────────────────┐
│                  VaultClient                    │
├─────────────────────────────────────────────────┤
│  ┌─────────┐  ┌─────────┐  ┌─────────┐         │
│  │ KeySlot │  │ KeySlot │  │ KeySlot │  ...    │
│  │ nonce=5 │  │ nonce=3 │  │ nonce=7 │         │
│  │ flight=2│  │ flight=0│  │ flight=1│         │
│  └─────────┘  └─────────┘  └─────────┘         │
│       │            ▲            │              │
│       │      (selected)         │              │
│       └────────────┴────────────┘              │
│           Least-loaded selection               │
└─────────────────────────────────────────────────┘
```

**Selection Algorithm:**
1. Filter to healthy keys only
2. Find keys with minimum in-flight transaction count
3. Round-robin tiebreaker among candidates

### Per-Key Nonce Management

Each `KeySlot` maintains isolated nonce state with TTL-based block hash caching:

```rust
pub struct KeySlot {
    signer: ZeroizingSigner,           // Auto-zeroized on drop
    tx_lock: Mutex<()>,                // Per-key serialization
    nonce_state: Mutex<NonceState>,    // (nonce, block_hash, expires_at)
    healthy: AtomicBool,
    in_flight: AtomicU32,
    total_txs: AtomicU64,
    total_failures: AtomicU64,
}
```

On `InvalidNonce` error:
1. Invalidate cached nonce state
2. Select a different key from the pool
3. Refetch nonce from RPC
4. Retry transaction (up to `max_nonce_retries`)

### View Caching

Expensive view calls are cached with configurable TTL:

```rust
ViewCacheKey {
    account_id: String,
    method: String,
    args: Vec<u8>,  // Serialized
}
```

Default: 2000 entries, 2 second TTL.

### Secret Key Security

Keys are wrapped in a zeroizing container that clears memory on drop:

```rust
impl Drop for ZeroizingSigner {
    fn drop(&mut self) {
        match &mut self.0.secret_key {
            SecretKey::ED25519(k) => k.0.zeroize(),
            SecretKey::SECP256K1(k) => k.secret_bytes().zeroize(),
        }
    }
}
```

---

## Generator Library

The `impl_vault_methods!` macro generates 50+ vault methods from a single invocation, eliminating boilerplate.

### Method Categories

**Simple View Methods** (return `U128`):
```rust
get_total_assets()
get_last_total_assets()
get_idle_balance()
get_total_supply()
get_max_deposit()
get_max_single_market_deposit()
```

**Parameterized View Methods**:
```rust
convert_to_shares(assets)
convert_to_assets(shares)
preview_deposit(assets)
preview_mint(shares)
preview_withdraw(assets)
preview_redeem(shares)
```

**Typed View Methods** (complex return types):
```rust
get_configuration()        -> VaultConfiguration
get_fees()                 -> Fees
get_restrictions()         -> Option<Restrictions>
build_real_assets_report() -> RealAssetsReport
```

**Call Methods**:
```rust
deposit(amount, receiver, deposit_yocto)
withdraw(assets, receiver, deposit_yocto)
redeem(shares, receiver, deposit_yocto)
allocate(delta)
refresh_markets(markets)
execute_withdrawal(route)
set_fees(fees)
// ... 40+ more
```

### Macro Requirements

Any struct using `impl_vault_methods!` must provide:

```rust
impl MyClient {
    fn vault_view_u128(&self, method: &str) -> Result<U128, Error>;
    fn vault_call(&self, method: &str, args: Value) -> Result<(), Error>;
    fn vault_call_with(&self, method: &str, args: Value, deposit: u128) -> Result<(), Error>;
    fn vault_call_returning<T>(&self, method: &str, args: Value) -> Result<T, Error>;
    fn view<T>(&self, account: &str, method: &str, args: Value) -> Result<T, Error>;
    fn near_id(&self, id: &AccountId) -> NearAccountId;
    fn vault(&self) -> &NearAccountId;
}
```

---

## Prepared Flows

### Deposit Flow

```python
# Preview how many shares you'll receive
shares = await client.preview_deposit(amount)

# Execute deposit
await client.deposit(amount, receiver_id, storage_deposit_yocto)
```

### Withdraw Flow

Two-phase process for safety:

```python
# 1. Preview withdrawal
preview = await client.preview_withdraw(assets)

# 2. Submit withdrawal request
await client.withdraw(assets, receiver_id, deposit_yocto)

# 3. Execute when ready (may require multiple calls for large amounts)
await client.execute_withdrawal(route)
```

### Redeem Flow

Same as withdraw, but input is shares instead of assets:

```python
assets = await client.preview_redeem(shares)
await client.redeem(shares, receiver_id, deposit_yocto)
await client.execute_withdrawal(route)
```

### Refresh Markets

Update the vault's view of underlying market assets:

```python
# Refresh specific markets
report = await client.refresh_markets([market_id_1, market_id_2])

# Or refresh all
report = await client.refresh_all_markets()

print(f"Total assets: {report.total_assets}")
print(f"Refreshed at: {report.refreshed_at_ns}")
for market in report.per_market:
    print(f"  {market.market_id}: {market.assets}")
```

### Allocation (Curator Operations)

```python
from templar_vault_client import AllocationDelta, Delta

# Supply to a market
await client.allocate(AllocationDelta.Supply(Delta(
    market=market_id,
    amount=amount
)))

# Withdraw from a market
await client.allocate(AllocationDelta.Withdraw(Delta(
    market=market_id,
    amount=amount
)))
```

---

## ABI Generation

The SDK generates TypeScript types from the vault contract ABI.

### Generation Flow

```bash
make abi
```

This runs:
1. `cargo near abi` on the vault contract
2. Copies `vault.abi.json` to `web/src/abi/generated/`
3. Runs `npm run generate-types` for TypeScript bindings

### Output

```
web/
├── src/
│   ├── abi/
│   │   └── generated/
│   │       └── vault.abi.json
│   └── types/
│       └── vault.ts  # Generated TypeScript types
```

---

## Configuration

### VaultClientConfig

```python
from templar_vault_client import VaultClientConfig, RetryConfig

config = VaultClientConfig(
    timeout_seconds=60,           # RPC timeout
    retry=RetryConfig(
        max_attempts=5,
        initial_backoff_ms=100,
        max_backoff_ms=5000,
    ),
    max_nonce_retries=5,          # InvalidNonce retry limit
    block_hash_ttl_seconds=30,    # Block hash cache TTL
    view_cache_capacity=2000,     # View cache max entries
    view_cache_ttl_seconds=2,     # View cache TTL
)
```

### Client Construction

**Single Key**:
```python
client = VaultClient.new_single_key_default(
    rpc_url="https://rpc.mainnet.near.org",
    vault=AccountId("vault.near"),
    credential=KeyCredential(
        account_id=AccountId("signer.near"),
        secret_key="ed25519:..."
    )
)
```

**Multi-Key Pool**:
```python
credentials = [
    KeyCredential(AccountId("key1.near"), "ed25519:..."),
    KeyCredential(AccountId("key2.near"), "ed25519:..."),
    KeyCredential(AccountId("key3.near"), "ed25519:..."),
]

client = VaultClient.new_key_pool(
    rpc_url="https://rpc.mainnet.near.org",
    vault=AccountId("vault.near"),
    credentials=credentials,
    config=config
)
```

---

## Health Monitoring

```python
health = client.get_pool_health()

print(f"Total keys: {health.total_keys}")
print(f"Healthy keys: {health.healthy_keys}")
print(f"In-flight transactions: {health.total_in_flight}")

for key in health.keys:
    print(f"  {key.public_key[:20]}...")
    print(f"    Account: {key.account_id}")
    print(f"    Healthy: {key.is_healthy}")
    print(f"    In-flight: {key.in_flight}")
    print(f"    Total TXs: {key.total_transactions}")
    print(f"    Failures: {key.total_failures}")
```

---

## Error Handling

```python
from templar_vault_client import ErrorWrapper

try:
    await client.deposit(amount, receiver, deposit)
except ErrorWrapper.Timeout as e:
    # RPC timeout - transaction may or may not have succeeded
    pass
except ErrorWrapper.InvalidNonce:
    # Should not happen - retries are automatic
    pass
except ErrorWrapper.TransactionFailed as e:
    # Contract rejected the transaction
    print(f"TX failed: {e}")
except ErrorWrapper.Rpc as e:
    # RPC communication error
    pass
```

---

## Project Structure

```
contracts/client/vault/
├── src/
│   ├── lib.rs              # FFI exports, type definitions
│   ├── client.rs           # VaultClient wrapper
│   ├── methods.rs          # impl_vault_methods! macro
│   ├── lock_ext.rs         # Mutex/RwLock utilities
│   └── key_pool/
│       ├── mod.rs          # Module docs
│       ├── client.rs       # KeyPoolClient (core impl)
│       ├── pool.rs         # Least-loaded selection
│       ├── slot.rs         # Per-key nonce management
│       ├── health.rs       # Observability types
│       └── nonce.rs        # Nonce fetching
├── web/                    # TypeScript builder/executor
│   └── src/
│       ├── builder/        # Transaction builders (deposit, withdraw, refresh)
│       ├── executor/       # Transaction execution
│       ├── wallet/         # Wallet Selector adapter
│       ├── rpc/            # RPC utilities
│       └── policy/         # Gas/deposit policies
├── dist/                   # Generated FFI artifacts
│   ├── python/             # Python bindings + .so
│   └── web/                # TypeScript bindings
├── Cargo.toml
├── Makefile
├── uniffi-bindgen.rs       # Binding generator entry
└── smoke_test.py           # Python integration tests
```

---

## Web Package

The `web/` directory contains TypeScript utilities for frontend integration:

### Transaction Builders

```typescript
// web/src/builder/
deposit.ts   // Build deposit transactions
withdraw.ts  // Build withdraw transactions
refresh.ts   // Build refresh market transactions
```

### Wallet Integration

```typescript
// web/src/wallet/adapter.ts
// Adapts Wallet Selector for vault operations
```

### Executor

```typescript
// web/src/executor/
// Execute built transactions through wallet
```

These are the prepared flows for frontend - build transactions with proper gas/deposit, execute through Wallet Selector.

---

## Development

```bash
# Build Python bindings
make python

# Run smoke tests
make smoke-test

# Generate ABI + TypeScript types
make abi

# Full release build
make python MODE=release
```

---

## Summary

| Aspect | Implementation |
|--------|----------------|
| FFI Framework | UniFFI |
| Async Runtime | Tokio |
| Concurrency | Multi-key pool, least-loaded selection |
| Nonce Strategy | Per-key caching with TTL + InvalidNonce retry |
| View Caching | mini-moka, TTL-based |
| Key Security | Zeroizing drop |
| Code Generation | Macro-based (`impl_vault_methods!`) |
| Observability | Tracing + health reporting |
| Methods | 50+ generated via macro |

The SDK prioritizes production reliability for automated curator/allocator systems, with proper error handling, retry logic, and multi-key concurrency support.
