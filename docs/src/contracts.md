# Smart Contract Addresses

This page provides information about Templar Protocol smart contracts and how to interact with them.

## Deployed Addresses

### NEAR Mainnet
- **Registry**: `v1.tmplr.near`
- **Pyth Oracle**: `pyth-oracle.near` with asset ids from: `https://insights.pyth.network/price-feeds`
- **LST Oracle Adapter**: `lst.oracle.tmplr.near`

## Contract Architecture

### Registry Contract
The registry is the central contract that:
- Stores approved market contract versions
- Deploys new markets with specific configurations
- Maintains a list of all deployed markets

### Market Contracts
Market contracts are deployed dynamically through the registry. Each market represents a single asset pair (COLLATERAL → BORROW).

To get the current list of deployed markets:
```bash
near view <registry-address> list_deployments '{"offset": 0, "count": 100}'
```

### Oracle Contracts
- **Pyth Oracle**: Provides price feeds for assets ([Documentation](https://docs.pyth.network/))
- **LST Oracle Adapter**: Transforms base oracle prices for liquid staking tokens

## Interacting with Contracts

### Querying Market Information
```bash
# Get all deployed markets
near view <registry-address> list_deployments '{"offset": 0, "count": 100}'

# Get specific market deployment info
near view <registry-address> get_deployment '{"account_id": "<market-address>"}'

# Query market configuration
near view <market-address> get_configuration '{}'
```

### Contract Verification

All smart contracts use reproducible builds. To verify deployed code:

1. **Build locally**:
   ```bash
   cd contract/market
   cargo near build reproducible-wasm
   ```

2. **Compare with registry deployment records**:
   ```bash
   # Get deployment hash from registry  
   near view <registry-address> get_deployment '{"account_id": "<market-address>"}'
   ```

## Contract ABIs and Schemas

After building, contract WASMs are available in `target/near/`:
- **Market**: `target/near/templar_market_contract/templar_market_contract.wasm`
- **Registry**: `target/near/templar_registry_contract/templar_registry_contract.wasm`
- **LST Oracle**: `target/near/templar_lst_oracle_contract/templar_lst_oracle_contract.wasm`

## Getting Current Addresses

To list all deployed markets (subaccounts of `v1.tmplr.near`):
```bash
# Query all deployed markets from registry
near view v1.tmplr.near list_deployments '{"offset": 0, "count": 100}'
```

For additional protocol information, visit:
- **Official Website**: [templarfi.org](https://templarfi.org/)
- **Discord**: [Templar Protocol Discord](https://discord.gg/KAvMtYpbep)
- **Telegram**: [Templar Protocol Telegram](https://t.me/templarprotocol)
