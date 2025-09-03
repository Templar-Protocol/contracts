# Liquidate

Liquidation is the process by which the asset collateralizing certain positions may be reappropriated e.g. to recover assets for an undercollateralized position.

A liquidator is a third party willing to send a quantity of a market's borrow asset (usually a stablecoin) to the market in exchange for the amount of collateral asset supporting a specific account's position. As compensation for this service, the liquidator receives an exchange rate that is slightly better than the current rate. This difference in rates is called the "liquidator spread," and the maximum liquidator spread is configurable on a per-market basis.

A liquidator will follow this high-level workflow:

1. The liquidator obtains a list of accounts borrowing from the market by calling `list_borrows`.
1. The liquidator checks the status of each account by calling `get_borrow_status(account_id)`.
1. If an account's status is `Liquidation`, that means the liquidator can obtain a spread by sending an amount of borrow asset to the market. The maximum spread is `liquidation_maximum_spread` in [the market configuration](./index.md#configuration).
1. To perform the liquidation, the liquidator transfer-calls the appropriate amount of borrow asset to the market. That is to say, the liquidator calls `ft_transfer_call`/`mt_transfer_call` on the borrow asset's smart contract, specifying the market as the receiver. The `msg` parameter indicates 1) that the transfer is for a liquidation, and 2) which account is to be liquidated.

Thus, the arguments to a liquidation call might look something like this:

```json
{
  "amount": "<amount>",
  "msg": {
    "Liquidate": {
      "account_id": "<account-to-liquidate>"
    }
  },
  "receiver_id": "<market-id>"
}
```

<div class="warning">

It is the responsibility of the liquidator to calculate the optimal amount of tokens to attach to a liquidation call. The market will either completely accept or completely reject the liquidation attempt&mdash;no refunds!

</div>

## Example

```bash
near contract call-function as-transaction \
    <borrow-asset-contract-id> ft_transfer_call \
    json-args '{
        "receiver_id": "<market-id>",
        "amount": "<amount>",
        "msg": "{ \"Liquidate\": { \"account_id\": \"<account-to-liquidate>\" } }"
    }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as <account-id>
```
