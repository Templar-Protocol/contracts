# Markets

A single Templar market represents a pair of collateral and borrow assets, such as BTC/USDC for Bitcoin-collateralized USDC loans.

Suppliers may deposit borrow assets into the market, and their funds will earn yield from the protocol fees paid by borrowers. Borrowers may borrow available supply assets from the market, paying a variable interest rate based on the supply utilization rate.

Markets support NEAR fungible asset contracts implementing [the NEP-141 standard](https://nomicon.io/Standards/Tokens/FungibleToken/Core) or [the NEP-245 standard](https://nomicon.io/Standards/Tokens/MultiToken/Core). The borrow and collateral assets do not need to implement the same standard.

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
<!-- cmdrun near contract call-function as-read-only ibtc-usdc.v1.tmplr.near get_configuration json-args {}  network-config mainnet now -->
```

</details>

## Snapshots

Interest and yield on borrow and supply positions (respectively) are calculated using a market-snapshot system.

Every time a "time chunk" (wlog 1 hour, configurable) elapses, the contract takes a snapshot, recording such things as the total supply deposit, amount borrowed, timestamp, etc. Taking a snapshot is a relatively inexpensive process.

Whenever a borrow or supply position update requires, interest/yield calculations are triggered. (They can also be triggered explicitly using `harvest_yield()` and `accumulate_interest()`.) These calculations iterate from the snapshot at which the record was last updated until the most-recently-finalized snapshot.
