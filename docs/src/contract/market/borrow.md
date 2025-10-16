# Borrow { #borrow-1 }

Accounts may borrow assets from the market's supply.

Borrow positions must be collateralized with a minimum amount of collateral asset determined by the market's configuration.

## Deposit collateral

To add collateral to an account's position, transfer-call the market tokens with a `msg` of [`"Collateralize"`](../../../doc/templar_common/market/enum.DepositMsg.html#variant.Collateralize):

```bash
near contract call-function as-transaction \
    <collateral-asset-contract-id> ft_transfer_call \
    json-args '{
        "receiver_id": "<market-id>",
        "amount": "<amount>",
        "msg": "\"Collateralize\""
    }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as <account-id>
```

## Withdraw collateral

The collateral withdrawal process is relatively straightforward as compared to [the supply withdrawal process](supply.md#withdraw): simply call [`withdraw_collateral`](../../../doc/templar_common/market/trait.MarketExternalInterface.html#tymethod.withdraw_collateral), passing the amount of collateral asset tokens you wish to withdraw:

```bash
near contract call-function as-transaction \
    <market-id> withdraw_collateral \
    json-args '{ "amount": "<amount>" }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '0 NEAR' \
    sign-as <account-id>
```

While this process is simple, collateral can only be withdraw so long as the value of the remaining collateral continues to satisfy the market's [`borrow_mcr_maintenance`](../../../doc/templar_common/market/struct.MarketConfiguration.html#structfield.borrow_mcr_maintenance) requirement.

## Borrow

Once an account's position is collateralized, borrow asset can be withdrawn.

```bash
near contract call-function as-transaction \
    <market-id> borrow \
    json-args '{ "amount": "<amount>" }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '0 NEAR' \
    sign-as <account-id>
```

As long as the collateralization requirements are met, the borrow amount (minus fees) will be sent to the predecessor account.

## Repay

As long as an account has a liability (principal + interest/fees), some or all of their collateral will be locked so that it cannot be withdrawn.

To unlock the collateral, the account must repay its liability to the market.

To perform a repayment, transfer-call tokens to the market with a `msg` of [`"Repay"`](../../../doc/templar_common/market/enum.DepositMsg.html#variant.Repay):

```bash
near contract call-function as-transaction \
    <borrow-asset-contract-id> ft_transfer_call \
    json-args '{
        "receiver_id": "<market-id>",
        "amount": "<amount>",
        "msg": "\"Repay\""
    }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as <account-id>
```
