# Markets

A single Templar market represents a pair of collateral and borrow assets, such as BTC/USDC for Bitcoin-collateralized USDC loans.

Suppliers may deposit borrow assets into the market, and their funds will earn yield from the protocol fees paid by borrowers. Borrowers may borrow available supply assets from the market, paying a variable interest rate based on the supply utilization rate.

Markets support NEAR fungible asset contracts implementing the [NEP-141](https://nomicon.io/Standards/Tokens/FungibleToken/Core) standard or the [NEP-245](https://nomicon.io/Standards/Tokens/MultiToken/Core) standard. The borrow and collateral assets do not need to implement the same standard.

Broadly speaking, users can interact with markets in seven primary ways:

1. Deposit supply.
1. Withdraw supply.
1. Deposit collateral.
1. Withdraw collateral.
1. Borrow supply.
1. Repay supply.
1. Liquidate borrow position.
