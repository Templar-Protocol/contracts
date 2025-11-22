# Known Issues

## v1.1.0

### Static yield withdrawal rollback not written back to storage

If the token transfer initiated in `withdraw_static_yield` fails, the callback correctly calculates the rollback, but does not write it back to storage.

Fixed in [`eb622a0e14be166e84b7087752c49f3bbadf353a`](https://github.com/Templar-Protocol/contracts/commit/eb622a0e14be166e84b7087752c49f3bbadf353a).
