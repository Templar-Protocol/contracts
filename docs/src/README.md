# Notes

## Sending funds to the market contract

When sending funds to a contract, you must **call the asset contract's `*_transfer_call` function with the market as the `receiver_id`**. So, for a token contract that implements the NEP-141 (Fungible Token) standard, you must call `ft_transfer_call`, specifying the market account ID as the `receiver_id` argument. For a token contract that implements the NEP-245 (Multi Token) standard, you must call `mt_transfer_call`.

<div class="warning">

If the funds are not sent using a `*_transfer_call` function, the contract will not be able to respond to the transfer: the funds will not be tracked by the contract, they will not be added to the supply, and **the funds cannot be returned or withdrawn**.

</div>

## Contract interaction syntax

Contract interactions will be shown using [`near-cli-rs`](https://github.com/near/near-cli-rs) syntax. It can be installed via:

```bash
cargo install near-cli-rs
```

### Example

```bash
near contract call-function as-transaction \
    ibtc-usdc.v1.tmplr.near borrow \
    json-args '{ "amount": "1000" }' \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '0 NEAR' \
    sign-as account.near \
    network-config mainnet \
    sign-with-keychain \
    send
```

This command calls the function `borrow` on the contract `ibtc-usdc.v1.tmplr.near` with the arguments payload:

```json
{
  "amount": "1000"
}
```

Large numbers are serialized as strings instead of numerical literals to ensure that the precision limitations of JSON parsers do not affect the values. (See ["Notes on Serialization" on docs.near.org](https://docs.near.org/smart-contracts/anatomy/serialization#serializing-input--output).)

The command attaches 100 teragas units and 0 NEAR to the call, signs the transaction as `account.near` using a key saved to the local keychain, and sends the transaction to NEAR mainnet.

Please refer to [the user guide](https://github.com/near/near-cli-rs/blob/main/docs/GUIDE.en.md) for more details.
