# Supply

To add funds to a market's supply, send the funds using a callback-notifying `ft_transfer_call` or `mt_transfer_call` function call. Note that if the funds are not sent using a `*_call` function, the contract will not be able to respond to the transfer: the funds will not be tracked by the contract, they will not be added to the supply, and they cannot be returned or withdrawn.

For example:

```bash
near contract call-function as-transaction <borrow-asset-contract-id> ft_transfer_call \
    json-args '{
        "receiver_id": "<market-id>",
        "amount": "1",
        "msg": "\"Supply\""
    }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as <account-id>
```
