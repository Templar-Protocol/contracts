# Templar bots

## Liquidator Bot for Templar

This bot is designed to monitor Templars' lending markets on the Near blockchain and perform liquidations when borrowers fall below their collateralization ratio.
It uses near tooling to execute liquidations, transfers, signing etc...
The bot is built using the Near SDK, and it can be used as a running service.

The bot is structured into several components:

- `liquidator.rs`: Contains the Liquidator struct that handles the liquidation logic for a specific market and signer.
- `bin/liquidator-bot.rs`: An executable service that manages the liquidation process, running in a loop to check for liquidatable positions.
- `near.rs`: Contains the Near SDK logic and RPC calls to interact with the Near blockchain, including fetching prices, borrow positions, and updating prices.
- `lib.rs`: Defines network related configuration and constants used throughout the bot. This is a utility module that helps the bot to interact with the NEAR Blockchain and oracles.

Prerequisites:

- Rust (install via rustup)
- NEAR account
- NEAR CLI (for deploying and interacting with contracts)
- Deployed NEAR contracts for the lending protocol
- Oracle contract for price data

Running the Bot:

```bash
liquidator-service \
    --markets market1.testnet \
    --signer-key ed25519:\<YOUR_PRIVATE_KEY_HERE> \
    --signer-account liquidator.testnet \
    --asset usdc.testnet \
    --network testnet \
    --timeout 60 \
    --concurrency 10 \
    --interval 600
```

Arguments:

- `--markets`: A list of markets to monitor for liquidations (e.g., templar-market1.testnet).
- `--signer-key`: The private key of the signer account used to sign transactions.
- `--signer-account`: The NEAR account that will perform the liquidations (e.g., templar-liquidator.testnet).
- `--asset`: The asset to liquidate NEP-141 token account used for repayments (e.g., usdc.testnet).
- `--network`: The NEAR network to connect to (e.g., testnet).
- `--timeout`: The timeout for RPC calls in seconds (default is 60 seconds).
- `--concurrency`: The number of concurrent liquidation attempts (default is 10).
- `--interval`: The interval in seconds for the service to check for liquidatable positions (default is 600 seconds).

How it works:

1. The bot initializes a Liquidator object for each market specified in the `--markets` argument.
1. It continuously checks the status of borrowers in each market.
1. If a borrower is found to be liquidatable, it calculates the liquidation amount based on the borrower's collateral and debt.
1. It sends an `ft_transfer_call` RPC call to the smart contract to trigger the liquidation process.
1. The bot will repeat this process every `interval` seconds for the service, or run once for the cron job.
1. The bot logs the results of each liquidation attempt, including success or failure, and any relevant details about the borrower and market.
1. If the liquidation is successful, the bot updates the borrower's position and the market's state accordingly.
1. The bot handles errors and retries failed liquidation attempts based on the configured timeout and concurrency settings.
1. The bot can be monitored via logs or integrated with a monitoring system to alert on significant events, such as successful liquidations or errors.
1. The bot can be extended to support additional liquidation strategies.

Liquidation Logic:
The liquidation logic is encapsulated within the `Liquidator` object, which is responsible for:

- Checking a borrower's status to determine if they are below the required collateralization ratio.
- Calculating the liquidation amount based on the borrower's collateral and debt. (This calculation should be implemented by the liquidator according to their specific strategy or requirements.)

```rust
#[instrument(skip(self), level = "debug")]
fn liquidation_amount(
    &self,
    position: &BorrowPosition,
    _oracle_response: &OracleResponse,
) -> Result<U128, Error> {
    // TODO: Calculate optimal liquidation amount
    // For purposes of this example implementation we will just use the total borrow amount
    // Costs to take into account here are:
    //  - Liquidation spread
    //  - Gas fees
    //  - Price impact
    //  - Slippage
    //  - Possible flash loan fees
    // All of this would be used in calculating both the optimal liquidation amount and wether to
    // perform full or partial liquidation
    Ok(position.get_total_borrow_asset_liability().into())
}
```

- Sending the `ft_transfer_call` RPC call to the smart contract to trigger the liquidation process.
- Handling errors and retries for failed liquidation attempts.
- Logging the results of each liquidation attempt for monitoring and debugging purposes.
