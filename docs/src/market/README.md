# Markets

A single Templar market represents a pair of collateral and borrow assets, such as BTC/USDC for Bitcoin-collateralized USDC loans.

Suppliers may deposit borrow assets into the market, and their funds will earn yield from the protocol fees paid by borrowers. Borrowers may borrow available supply assets from the market, paying a variable interest rate based on the supply utilization rate.

Markets support NEAR fungible asset contracts implementing the [NEP-141](https://nomicon.io/Standards/Tokens/FungibleToken/Core) standard or the [NEP-245](https://nomicon.io/Standards/Tokens/MultiToken/Core) standard. The borrow and collateral assets do not need to implement the same standard.

## Interactions

Accounts can interact with markets in seven primary ways:

1. Deposit supply.
1. Withdraw supply.
1. Deposit collateral.
1. Withdraw collateral.
1. Borrow supply.
1. Repay supply.
1. Liquidate borrow position.

## Configuration

A market's configuration is immutable after deployment. It can be obtained from the market contract by calling the `get_configuration` function.

### Example

```bash
near contract \
    call-function as-read-only ibtc-usdc.v1.tmplr.near get_configuration \
    json-args {} \
    network-config mainnet \
    now
```

<details>
    <summary>Output</summary>

```json
{
  "borrow_asset": {
    "Nep141": "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1"
  },
  "borrow_asset_maximum_usage_ratio": "0.99000000000000000000000000000000000001",
  "borrow_interest_rate_strategy": {
    "Piecewise": {
      "base": "0",
      "optimal": "0.90000000000000000000000000000000000001",
      "rate_1": "0.08888888888888888888888888888888888889",
      "rate_2": "2.40000000000000000000000000000000000001"
    }
  },
  "borrow_maximum_duration_ms": null,
  "borrow_mcr_liquidation": "1.19999999999999999999999999999999999999",
  "borrow_mcr_maintenance": "1.25",
  "borrow_origination_fee": {
    "Proportional": "0.00099999999999999999999999999999999999"
  },
  "borrow_range": {
    "maximum": null,
    "minimum": "1"
  },
  "collateral_asset": {
    "Nep245": {
      "contract_id": "intents.near",
      "token_id": "nep141:btc.omft.near"
    }
  },
  "liquidation_maximum_spread": "0.05000000000000000000000000000000000001",
  "price_oracle_configuration": {
    "account_id": "pyth-oracle.near",
    "borrow_asset_decimals": 6,
    "borrow_asset_price_id": "eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a",
    "collateral_asset_decimals": 8,
    "collateral_asset_price_id": "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43",
    "price_maximum_age_s": 60
  },
  "protocol_account_id": "revenue.tmplr.near",
  "supply_range": {
    "maximum": null,
    "minimum": "40000"
  },
  "supply_withdrawal_fee": {
    "behavior": "Fixed",
    "duration": "0",
    "fee": {
      "Flat": "0"
    }
  },
  "supply_withdrawal_range": {
    "maximum": null,
    "minimum": "40000"
  },
  "time_chunk_configuration": {
    "BlockTimestampMs": {
      "divisor": "600000"
    }
  },
  "yield_weights": {
    "static": {
      "revenue.tmplr.near": 1,
      "rewards.tmplr.near": 1
    },
    "supply": 1
  }
}
```

</details>
