# Templar Protocol Smart Contracts

[![Test](https://github.com/Templar-Protocol/contracts/actions/workflows/test.yml/badge.svg)](https://github.com/Templar-Protocol/contracts/actions/workflows/test.yml)
[![Coverage](https://codecov.io/gh/Templar-Protocol/contracts/branch/dev/graph/badge.svg)](https://codecov.io/gh/Templar-Protocol/contracts)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Templar Protocol is a chain-agnostic overcollateralized lending DeFi protocol.

## Structure

- `common` \
  Most of the pure logic governing the protocol.
- `contract` \
  Protocol smart contract source code.
  - `market` \
    Smart contract representing a single asset pair `COLLATERAL`&rarr;`BORROW`.
  - `registry` \
    Smart contract that manages and deploys market WASM binaries.
- `docs` \
  Detailed documentation pertaining to specific use-cases.
- `mock` \
  Smart contracts for testing.
- `script` \
  Shell scripts for testing and CI/CD.
- `service` \
  Standalone executable services.
  - `relayer` \
    The relayer pays for transaction fees on behalf of users who submit signed delegate actions.

## Notes

- The Soroban curator vault persists kernel↔Soroban address mappings in storage for effect execution; mappings are loaded automatically when executing effects.
- The Soroban entrypoints register caller/receiver addresses automatically. Use the curator-only `register_address` entrypoint to persist mappings for fee recipients or any address not provided during a call.
- The Soroban curator vault now settles queued withdrawals against whatever idle assets are currently available, burning shares proportionally and refunding the remainder.
- Legacy/dust withdrawals with `expected_assets == 0` are skipped and escrowed shares are refunded.

## Build and run tests

```bash
./script/test.sh
```

## Links

- [Website](https://templarfi.org/)
- [Testnet](https://testnet.templarfi.org/)
- [X (Twitter)](https://x.com/TemplarProtocol)
- [Discord](https://discord.gg/KAvMtYpbep)
- [Telegram](https://t.me/templarprotocol)
