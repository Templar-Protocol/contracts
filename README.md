# Templar Protocol

[![Test](https://github.com/Templar-Protocol/contracts/actions/workflows/test.yml/badge.svg)](https://github.com/Templar-Protocol/contracts/actions/workflows/test.yml)
[![Coverage](https://codecov.io/gh/Templar-Protocol/contracts/branch/dev/graph/badge.svg)](https://codecov.io/gh/Templar-Protocol/contracts)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Templar Protocol is an overcollateralized lending protocol. This repository contains the core smart contracts, shared protocol logic, operator services, client libraries, test utilities, and supporting tooling used across the protocol.

## Structure

- `common`  
  Shared protocol logic used by contracts, services, and tests.
- `contract`  
  Deployable smart contracts and contract-specific crates.
- `service`  
  Standalone off-chain services and bots.
- `tools`  
  Operator and developer command-line tools.
- `client`  
  Client libraries and SDKs.
- `mock`  
  Mock contracts used in tests.
- `test-utils`  
  Shared test harness utilities.
- `universal-account`  
  Shared universal-account crate.
- `fuzz`  
  Fuzz targets and related utilities.
- `docs`  
  Protocol and operational documentation.
- `audits`  
  Auditor-facing notes and known-issue references.
- `script`  
  Repository scripts for testing and CI workflows.

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
