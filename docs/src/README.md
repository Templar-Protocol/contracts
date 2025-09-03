# Templar Protocol

## Sending funds to the market contract

When sending funds to a contract, you must **call the asset contract's `*_transfer_call` function with the market as the `receiver_id`**. So, for a token contract that implements the NEP-141 (Fungible Token) standard, you must call `ft_transfer_call`, specifying the market account ID as the `receiver_id` argument. For a token contract that implements the NEP-245 (Multi Token) standard, you must call `mt_transfer_call`.

<div class="warning">

If the funds are not sent using a `*_transfer_call` function, the contract will not be able to respond to the transfer: the funds will not be tracked by the contract, they will not be added to the supply, and **the funds cannot be returned or withdrawn**.

</div>

## Notes

- Contract interactions will be shown using [`near-cli-rs`](https://github.com/near/near-cli-rs) syntax. It can be installed via:
  ```bash
  cargo install near-cli-rs
  ```
