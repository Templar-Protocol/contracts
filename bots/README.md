# Templar bots

## Liquidator Bot for Templar

This bot is designed to monitor Templars' lending markets on the Near blockchain and perform liquidations when borrowers fall below their collateralization ratio.
It uses near tooling to execute liquidations, transfers, signing etc...
The bot is built using the Near SDK, and it can be used as a running service.

The bot is structured into several components:

- `liquidator.rs`: Contains the Liquidator struct that handles the liquidation logic for a specific market and signer.
- `bin/liquidator-bot.rs`: An executable service that manages the liquidation process, running in a loop to check for liquidatable positions.
- `near.rs`: Contains the Near SDK logic and RPC calls to interact with the Near blockchain, including fetching prices, borrow positions, and updating prices.
- `swap.rs`: Contains the implementation for swapping assets dependent on the backend used (Rhea Finance, NEAR Intents).
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
    --swap rhea-swap \
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
- `--swap`: The swap to use for exchanging into the the desired asset (e.g., rhea-swap).
- `--network`: The NEAR network to connect to (e.g., testnet).
- `--timeout`: The timeout for RPC calls in seconds (default is 60 seconds).
- `--concurrency`: The number of concurrent liquidation attempts (default is 10).
- `--interval`: The interval in seconds for the service to check for liquidatable positions (default is 600 seconds).

How it works:

1. The bot initializes a Liquidator object for each market specified in the `--markets` argument.
1. It continuously checks the status of borrowers in each market.
1. If a borrower is found to be liquidatable, it calculates the liquidation amount based on the borrower's collateral and debt.
1. It sends an `ft_transfer_call` RPC call to the smart contract to trigger the liquidation process.
1. The bot will repeat this process every `interval` seconds.
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
async fn liquidation_amount(
    &self,
    position: &BorrowPosition,
    oracle_response: &OracleResponse,
    configuration: MarketConfiguration,
) -> LiquidatorResult<(U128, U128)> {
    // TODO: Calculate optimal liquidation amount
    // For purposes of this example implementation we will just use the minimum acceptable
    // liquidation amount.
    // Costs to take into account here are:
    //  - Gas fees
    //  - Price impact
    //  - Slippage
    // All of this would be used in calculating both the optimal liquidation amount and wether to
    // perform full or partial liquidation
    let borrow_asset = &configuration.borrow_asset;
    let collateral_asset = &configuration.collateral_asset;
    let price_pair = configuration
        .price_oracle_configuration
        .create_price_pair(oracle_response)?;
    let min_liquidation_amount = configuration
        .minimum_acceptable_liquidation_amount(position.collateral_asset_deposit, &price_pair)
        .ok_or_else(|| {
            LiquidatorError::MinimumLiquidationAmountError(
                "Failed to calculate minimum acceptable liquidation amount".to_owned(),
            )
        })?;
    // Here we would check a quote for the swap to ensure desired profit margin is met
    let quote_to_liquidate = self
        .swap
        .quote(
            &self.asset,
            &borrow_asset.clone().into_nep141().ok_or_else(|| {
                LiquidatorError::StandardSupportError(
                    "Only NEP-141 borrow assets supported".to_owned(),
                )
            })?,
            min_liquidation_amount.into(),
        )
        .await
        .map_err(LiquidatorError::QuoteError)?;
    let _quote_after_liquidate = self
        .swap
        .quote(
            // TODO: Enable multitoken swaps
            &collateral_asset.contract_id(),
            &self.asset,
            position.collateral_asset_deposit.into(),
        )
        .await
        .map_err(LiquidatorError::QuoteError)?;
    Ok((quote_to_liquidate, min_liquidation_amount.into()))
}
```

- Sending the `ft_transfer_call` RPC call to the borrow asset contract to trigger liquidation.
- Handling errors and retries for failed liquidation attempts.
- Logging the results of each liquidation attempt for monitoring and debugging purposes.

## Key snippets

### Getting a market configuration

```rust
#[instrument(skip(self), level = "debug")]
async fn get_configuration(&self) -> LiquidatorResult<MarketConfiguration> {
    view(
        &self.client,
        self.market.clone(),
        "get_configuration",
        json!({}),
    )
    .await
    .map_err(LiquidatorError::GetConfigurationError)
}
```

The liquidator will fetch the configuration for the given market in order to asses how to run the liquidations (i.e. which price oracle to query, which assets to send/swap...).

### Getting oracle prices

```rust
#[instrument(skip(self), level = "debug")]
async fn get_oracle_prices(
    &self,
    oracle: AccountId,
    price_ids: &[PriceIdentifier],
    age: u32,
) -> LiquidatorResult<OracleResponse> {
    view(
        &self.client,
        oracle,
        "list_ema_prices_no_older_than",
        json!({ "price_ids": price_ids, "age": age }),
    )
    .await
    .map_err(LiquidatorError::PriceFetchError)
}
```

The liquidator will fetch the price data from the oracle contract in order to execute the liquidation and gauge whether the liquidation is profitable.

### Getting the borrow positions for a market

```rust
#[instrument(skip(self), level = "debug")]
async fn get_borrows(&self) -> LiquidatorResult<BorrowPositions> {
    let mut all_positions: BorrowPositions = HashMap::new();
    let page_size = 100;
    let mut current_offset = 0;

    loop {
        let params = json!({
            "offset": current_offset,
            "count": page_size,
        });

        let page = view::<BorrowPositions>(
            &self.client,
            self.market.clone(),
            "list_borrow_positions",
            params,
        )
        .await
        .map_err(LiquidatorError::ListBorrowPositionsError)?;

        let fetched = page.len();

        if fetched == 0 {
            break;
        }

        all_positions.extend(page);
        current_offset += fetched;

        if fetched < page_size {
            break;
        }
    }

    Ok(all_positions)
}
```

The liquidator will query the market contract for all the borrow positions so that we can check each position for status.

### Getting the borrow status

```rust
#[instrument(skip(self), level = "debug")]
async fn get_borrow_status(
    &self,
    borrow: AccountId,
    oracle_response: &OracleResponse,
) -> LiquidatorResult<Option<BorrowStatus>> {
    view(
        &self.client,
        self.market.clone(),
        "get_borrow_status",
        &json!({
            "account_id": borrow,
            "oracle_response": oracle_response,
        }),
    )
    .await
    .map_err(LiquidatorError::FetchBorrowStatus)
}
```

The liquidator chech for the borrow status to know whether to run a liquidation in case of a `BorrowStatus::Liquidation` status.

### Getting a swap quote

```rust
async fn quote(&self, from: &AccountId, to: &AccountId, amount: U128) -> RpcResult<U128> {
    let response: QuoteResponse = view(
        &self.client,
        self.contract.clone(),
        "quote_by_output",
        &QuoteRequest::new(from.clone(), to.clone(), amount),
    )
    .await?;
    Ok(response.amount)
}
```

When we need to swap assets, we want to get a quote on the swap for the given value so that we can better calculate the profitability of a liquidation.

### Creating the liquidation transaction

```rust
fn create_transfer_tx(
    &self,
    borrow: &AccountId,
    liquidation_amount: U128,
    nonce: u64,
    block_hash: CryptoHash,
) -> LiquidatorResult<Transaction> {
    let msg = serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
        account_id: borrow.clone(),
    }))?;

    Ok(Transaction::V0(TransactionV0 {
        nonce,
        receiver_id: self.asset.clone(),
        block_hash,
        signer_id: self.signer.account_id.clone(),
        public_key: self.signer.public_key().clone(),
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer_call".to_string(),
            args: serialize_and_encode(json!({
                "receiver_id": self.market,
                "amount": liquidation_amount,
                "msg": msg,
            })),
            gas: DEFAULT_GAS,
            deposit: NearToken::from_yoctonear(1).as_yoctonear(),
        }))],
    }))
}
```

The liquidator creates a function call for transferring the given amount to the market contract with a `LiquidateMsg` in the `msg` field in order
to trigger the liquidation as part of the handler for `ft_transfer_call` (which triggers a function call after executing a transfer on the asset
contract).
