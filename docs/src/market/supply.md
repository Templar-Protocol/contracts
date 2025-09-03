# Supply

Accounts may deposit assets to the market's supply to earn yield.

## Deposit

To add funds to a market's supply, send the funds using a callback-notifying [`ft_transfer_call`](https://docs.near.org/primitives/ft#attaching-fts-to-a-call) or `mt_transfer_call` function call.

<div class="warning">

If the funds are not sent using a `*_transfer_call` function, the contract will not be able to respond to the transfer: the funds will not be tracked by the contract, they will not be added to the supply, and **the funds cannot be returned or withdrawn**.

</div>

For example:

```bash
near contract call-function as-transaction \
    <borrow-asset-contract-id> ft_transfer_call \
    json-args '{
        "receiver_id": "<market-id>",
        "amount": "<amount>",
        "msg": "\"Supply\""
    }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as <account-id>
```

## Withdraw

Since borrowers borrow the assets that suppliers have supplied, when a supplier wishes to withdraw their supply, there might not be enough available to withdraw at that time. However, through fees, interest, etc., as time passes, borrow assets should become available to withdraw again.

Because the market may not have sufficient borrow asset liquidity when a supplier wishes to withdraw, the market uses a queue-based withdrawal system.

In order to withdraw supply from the market, a supplier must first enter the supply withdrawal queue with their withdrawal request:

```bash
near contract call-function as-transaction \
    <market-id> create_supply_withdrawal_request \
    json-args '{"amount": "<amount>"}' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '0 NEAR' \
    sign-as <account-id>
```

Now the account has a position in the queue. Should the account wish to update the amount of the request, it can call `create_supply_withdrawal_request` again, however, this will also reset its position to the end of the queue.

A supply withdrawal request can be cancelled via `cancel_supply_withdrawal_request`.

In order for an account's supply withdrawal request to be fulfilled, all of the requests that are ahead of it in the queue must be fulfilled first.

To execute the next withdrawal request, use the `execute_next_supply_withdrawal_request` function:

```bash
near contract call-function as-transaction \
    <market-id> execute_next_supply_withdrawal_request \
    json-args '{}' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '0 NEAR' \
    sign-as <account-id>
```

This function is not permissioned; anyone may call it to advance the withdrawal queue.
