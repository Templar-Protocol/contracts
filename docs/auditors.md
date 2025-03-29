# For auditors

The SOW is `common` and `contract/market/src`.

This project is a cross-chain DeFi lending protocol.

To implement cross-chain behavior, it relies on NEAR Protocol's chain abstraction technologies, including the [NEAR MPC signer](https://github.com/near/mpc).

> [!NOTE]
> The single-chain version of the contract does not use any multichain technologies.

The contract relies on EMA prices from the local [Pyth oracle](https://www.pyth.network/). On testnet, the address is `pyth-oracle.testnet`, and on mainnet `pyth-oracle.near`.

## Snapshots

Interest and yield on borrow and supply positions (respectively) are calculated using a market-snapshot system.

Every time a "time chunk" (wlog 1 hour, configurable) elapses, the contract takes a snapshot, recording such things as the total supply deposit, amount borrowed, timestamp, etc. Taking a snapshot is a relatively inexpensive process.

Whenever a borrow or supply position update requires, interest/yield calculations are triggered. (They can also be triggered explicitly using `harvest_yield()` and `accumulate_interest()`.) These calculations iterate from the snapshot at which the record was last updated until the most-recently-finalized snapshot.
