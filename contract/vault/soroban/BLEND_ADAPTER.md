# Blend Adapter Design (Soroban)

Goal: map Blend v2 pool interfaces onto the Templar Soroban `SorobanMarketAdapter`
for local market supply/withdraw/position reads.

## References (local clones)
- Blend contracts v2: `/tmp/blend-contracts-v2`
  - Pool client: `/tmp/blend-contracts-v2/pool/src/contract.rs`
  - Request types: `/tmp/blend-contracts-v2/pool/src/pool/actions.rs`
  - Reserve data: `/tmp/blend-contracts-v2/pool/src/storage.rs`
  - Pool factory: `/tmp/blend-contracts-v2/pool-factory/src/pool_factory.rs`
- Blend SDK: `/tmp/blend-contract-sdk` (contractimport client)

## Required Addresses
- `BLEND_POOL_ID`: pool contract address (required)
- `BLEND_FACTORY_ID`: pool factory address (optional, for validation)
- `BLEND_ORACLE_ID`: oracle used by the pool (only needed if deploying via factory)
- Asset contract addresses for reserves the vault will allocate into

## Adapter Mapping

### Templar `SorobanMarketAdapter` API
```
fn supply(env, asset, amount) -> Result<(), RuntimeError>
fn withdraw(env, asset, amount) -> Result<(), RuntimeError>
fn total_assets(env, asset) -> Result<i128, RuntimeError>
```

### Blend Pool API (from PoolClient)
- `submit(from, spender, to, requests)`
- `submit_with_allowance(from, spender, to, requests)`
- `get_reserve(asset) -> Reserve`
- `get_positions(address) -> Positions`
- `get_reserve_list() -> Vec<Address>`
- `get_config()`, `get_admin()`

### Request Types (Blend)
`Request` fields: `{ request_type: u32, address: Address, amount: i128 }`

`RequestType` (u32):
```
Supply=0, Withdraw=1, SupplyCollateral=2, WithdrawCollateral=3,
Borrow=4, Repay=5, FillUserLiquidationAuction=6, FillBadDebtAuction=7,
FillInterestAuction=8, DeleteLiquidationAuction=9
```

### Suggested Call Flow

#### Supply
- Use `submit` with:
  - `from = vault_contract_address`
  - `spender = vault_contract_address` (vault holds assets)
  - `to = vault_contract_address`
  - `requests = [Request { request_type: Supply, address: asset, amount }]`
- Alternate: `submit_with_allowance` if spender is a different address using
  `transfer_from` allowance.

#### Withdraw
- Use `submit` with:
  - `from = vault_contract_address`
  - `spender = vault_contract_address`
  - `to = vault_contract_address`
  - `requests = [Request { request_type: Withdraw, address: asset, amount }]`

#### Total Assets (principal + interest)
- `reserve = pool.get_reserve(asset)`
- `positions = pool.get_positions(vault_contract_address)`
- Map `asset -> reserve_index` using `reserve.config.index`
- Read b-token balance from `positions.supply.get(index)` (and/or collateral if used)
- Convert b-token balance to underlying using reserve data:
  - `assets = b_tokens * reserve.data.b_rate / 1e12`
  - `SCALAR_12 = 1_000_000_000_000`
- If adapter supplies collateral instead of supply, use `positions.collateral`.

Note: Blend positions are stored per reserve index, not per asset. Use
`get_reserve(asset)` to get the index and look up the balance in `Positions`.

## Pool Factory (optional)
- `PoolFactoryClient::deploy(admin, name, salt, oracle, backstop_take_rate, max_positions, min_collateral) -> Address`
- `PoolFactoryClient::is_pool(pool_id) -> bool`

If we include factory validation, we can check `is_pool` on adapter init.

## Integration Notes
- We should depend on `blend-contract-sdk` (contractimport WASM) for `pool::Client`.
- This avoids pulling in unpublished `pool` crate; it provides client types + XDR.
- We can define a small `BlendMarketAdapter` with a `pool_id` address and
  optionally `factory_id` for validation.
- For tests, use `blend-contract-sdk` `testutils` fixture to deploy a mock pool.

## Open Questions
- Do we need to use `SupplyCollateral` (collateralized) instead of `Supply`?
  For a vault that never borrows, plain `Supply` should suffice.
- Should adapter enforce pool status (e.g., `update_status`/`set_status`) or
  leave that to governance/setup scripts?
- Is oracle integration required for vault accounting? Current total_assets
  uses reserve b_rate only (no price feed required).
