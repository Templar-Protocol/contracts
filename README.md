# Templar Protocol Smart Contracts

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

## Security Reporting

Send security reports to [security@templarprotocol.com](mailto:security@templarprotocol.com) or contact [@peer2f00l on Telegram](https://t.me/peer2f00l).
