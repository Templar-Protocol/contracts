# Glossary

This glossary provides definitions for key terms used throughout the Templar Protocol documentation and smart contracts.

## A

**APY (Annual Percentage Yield)**: The total return on an investment over one year. In Templar, this represents the effective yearly return for suppliers or the cost for borrowers.

**Asset Pair**: The combination of collateral asset and borrow asset that defines a market (e.g., BTC/USDC means Bitcoin collateral, USDC borrowing).

## B

**Borrow Asset**: The token that users can borrow from the market. Typically a stablecoin like USDC, but can be any supported token.

**Borrow Position**: A user's borrowing account containing collateral deposits, borrowed amounts, accumulated interest, and current status.

**Borrower**: A user who deposits collateral and borrows assets from the market, paying interest on the borrowed amount.

## C

**Collateral Asset**: The token deposited by borrowers to secure their loans. Must be worth more than the borrowed amount due to over-collateralization requirements.

**Collateralization Ratio (CR)**: The ratio of collateral value to borrowed value. A 150% ratio means $150 of collateral backs $100 of debt.

**Compounding**: The process of reinvesting earned yield to generate additional returns over time.

## D

**Debt**: The total amount owed by a borrower, including principal plus accumulated interest and fees.

## E

**EMA**: Exponentially-weighted moving average. A time-series smoothing technique that favors recency.

## F

**FMV (Fair Market Value)**: The current market price of an asset as determined by oracle price feeds.

## G

**Gas**: The computational cost for executing transactions on the NEAR blockchain.

## H

**Harvest Yield**: The action of claiming accumulated yield from a supply position, which can then be withdrawn.

## I

**Interest Accumulation**: The process of calculating and adding accrued interest to a borrower's total liability.

## L

**Lending**: The general practice of providing assets to borrowers in exchange for interest payments. In Templar, suppliers lend to the market pool.

**Liability**: The total debt owed by a borrower, including principal, accumulated interest, and fees.

**Liquidation**: The forced sale of a borrower's collateral when their position becomes undercollateralized or expires.

**Liquidator**: A third party who performs liquidations by repaying part of a borrower's debt in exchange for discounted collateral.

**Liquidator Spread**: The discount liquidators receive when purchasing collateral, serving as incentive for providing the liquidation service.

**Liquidity**: The availability of assets in the market for borrowing or withdrawal. When liquidity is low, withdrawal requests may need to wait in the queue.

**Liquidity Pool**: The combined supply of assets deposited by all suppliers in a market, available for borrowers to access.

## M

**Market**: A smart contract managing lending and borrowing for a specific asset pair (e.g., BTC/USDC market).

**Maximum Usage Ratio**: The maximum percentage of supplied assets that can be borrowed from a market, preventing over-utilization and maintaining liquidity reserves.

**MCR (Minimum Collateralization Ratio)**: The minimum ratio of collateral value to borrowed value required to maintain a position. Different MCR levels trigger maintenance requirements or liquidation.

**MCR Liquidation**: The minimum collateralization ratio below which a position becomes eligible for liquidation.

**MCR Maintenance**: The minimum collateralization ratio required for new borrows or collateral withdrawals.

## N

**NEAR Intents**: A NEAR Protocol feature that allows users to express desired outcomes (intents) that can be fulfilled by solvers, enabling more flexible and efficient transaction execution. See the [official NEAR Intents documentation](https://docs.near.org/chain-abstraction/intents/overview).

**NEP-141**: The NEAR Protocol standard for fungible tokens, similar to Ethereum's ERC-20. See the [official NEP-141 specification](https://nomicon.io/Standards/Tokens/FungibleToken/Core).

**NEP-245**: The NEAR Protocol standard for multi-token contracts, similar to Ethereum's ERC-1155. See the [official NEP-245 specification](https://nomicon.io/Standards/Tokens/MultiToken/Core).

## O

**Oracle**: A service providing real-time price data for assets, essential for calculating collateralization ratios and liquidations.

**Origination Fee**: A fee charged when creating a new borrow position, can be flat amount or percentage-based.

**Over-collateralization**: The requirement for borrowers to deposit collateral worth more than the borrowed amount, providing a safety buffer against price volatility.

## P

**Partial Liquidation**: Liquidating only enough collateral to bring a position back to the maintenance MCR, rather than liquidating the entire position.

**Principal**: The original amount borrowed or supplied, excluding accumulated interest and fees.

**Protocol Revenue**: Fees collected by the protocol from borrowers and suppliers, distributed to suppliers and other accounts according to configured yield weights.

**Pyth Network**: A decentralized oracle network providing high-frequency price feeds for various assets.

## R

**Registry**: A smart contract that manages deployment and versioning of market contracts within the Templar Protocol.

**Repay**: The action of returning borrowed assets plus interest to reduce or eliminate a borrower's debt.

## S

**Snapshot**: A point-in-time record of market state including interest rates, asset amounts, and yield distribution.

**Stablecoin**: A cryptocurrency designed to maintain stable value, typically pegged to a fiat currency like USD. Commonly used as borrow assets in lending protocols.

**Static Yield**: A fixed allocation of market revenue to specific accounts, independent of their supply activity. Defined in the market's yield_weights configuration, static yield is distributed proportionally to designated accounts and can be withdrawn using the withdraw_static_yield function.

**Supply**: The total amount of assets deposited by suppliers that are available for borrowing in a market.

**Supplier**: A user who deposits assets into the market to earn yield from borrower interest payments.

**Supply Withdrawal Fee**: A fee charged when suppliers withdraw their assets from the market, configured per market to manage liquidity.

## T

**Time Chunk**: A configurable time period (based on blocks, epochs, or timestamps) that determines when new snapshots are created.

**Transfer Call**: A token transfer that includes data, allowing the receiving contract to execute logic based on the transfer.

## U

**Undercollateralized**: A borrow position where the collateral value falls below the required minimum ratio, making it eligible for liquidation.

**Utilization Rate**: The percentage of supplied assets currently borrowed. Calculated as: `borrowed_amount / total_supplied_amount`.

## W

**Withdrawal Queue**: A first-in-first-out system for processing supply withdrawals when market liquidity is insufficient.

## Y

**Yield**: The return earned by suppliers on their deposited assets, generated from borrower interest payments and fees.
