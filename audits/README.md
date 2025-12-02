# Readme for auditors

## Why are some fees persisted even if a borrow transfer fails?

NEAR is an asynchronous blockchain, meaning that a single NEAR transaction can take multiple blocks to complete. (Therefore, in regards to atomicity, a NEAR _receipt_ is a closer analogue to an EVM transaction than a NEAR _transaction_ is.)

Cross-contract calls, in particular, take multiple blocks to complete:

1. Contract A dispatches a call to Contract B.
2. Contract B executes and produces a return value.
3. Contract A consumes the return value.

...for a total of (at least) 3 receipts in this example, each executing in a different block.

When a borrower initiates a borrow, the funds leave the active supply while the market attempts to transfer them to the borrower. If the borrow _fails_, the market re-adds those tokens to the active supply. This might look something like the following sequence of operations:

1. `market.near` subtracts 10 tokens from the active supply and dispatches a transfer of funds by calling `usdc.near->ft_transfer(receiver_id: borrower.near, amount: 10)`.
2. `usdc.near` rejects the transfer for some reason (blacklist, storage opt-out, etc.).
3. `market.near` ingests the response from `usdc.near`, identifies the failed transfer, and re-adds 10 tokens to the active supply.

Therefore, there is some time between steps 1 and 3 during which 10 tokens have been removed from the active supply, preventing those tokens from earning yield for suppliers from another borrower. Therefore, despite the fact that the borrower did not ultimately receive the tokens, the market must still charge fees to the borrower so that the suppliers can earn their fair yield.

Note that if the market did _not_ charge these fees, this would allow attackers to lock quantities of the supply _without having to pay fees_ by repeatedly requesting borrows to accounts that are incapable of receiving tokens.

# Known Issues

## v1.1.0

### Static yield withdrawal rollback not written back to storage

If the token transfer initiated in `withdraw_static_yield` fails, the callback correctly calculates the rollback, but does not write it back to storage.

Fixed in [`eb622a0e14be166e84b7087752c49f3bbadf353a`](https://github.com/Templar-Protocol/contracts/commit/eb622a0e14be166e84b7087752c49f3bbadf353a).
