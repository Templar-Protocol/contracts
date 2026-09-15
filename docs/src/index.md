# Templar Protocol User Guide

This is a comprehensive user guide for the Templar Protocol smart contracts.

Templar is an overcollateralized lending protocol. Each market is an isolated, immutable smart contract that pairs one collateral asset with one borrow asset. Collateral held on other chains (Bitcoin, Ethereum, Stellar, XRP, Solana, and more) reaches NEAR through [NEAR Intents](https://docs.near.org/chain-abstraction/intents/overview), and curated vaults on Stellar route depositor liquidity into markets.

For definitions of key terms and concepts, refer to the [Glossary](./glossary.md).

## Quick Navigation

- **[Architecture Overview](./architecture.md)** - How markets, oracles, NEAR Intents, vaults, and services fit together, and the trust assumptions
- **[Smart Contract Addresses](./addresses.md)** - Official contract addresses and verification
- **[Risk Parameters](./risk-parameters.md)** - Per-market collateralization ratios, interest curves, fees, and oracle sources
- **[Deployment and Version Log](./release-log.md)** - Released contract builds, their hashes, and audit coverage
- **[Deploying a market](./deployments.md)** - Declarative market deployment with `tmplrmgr`
- **[Oracles](./oracles.md)** - Pyth and RedStone price feeds, proxy oracles, and circuit breakers
- **[Protocol Governance](./governance.md)** - Immutability, administrative controls, multisig, and timelocks
- **[Stellar Curated Vaults](./vaults.md)** - Shares, fees, withdrawals, caps, and roles for depositors
- **[Stellar Vault Curator Guide](./curator-guide.md)** - Deployment, governance, allocation, withdrawals, and keeper operations
- **[Security](./security-overview.md)** - Audits, formal verification, oracle safeguards, monitoring, and operational security
- **[Security Reporting](./security.md)** - Responsible disclosure
- **[Monitoring and Risk Management](./monitoring.md)** - Alerting, the live risk dashboard, and protocol health checks
- **[Testing and Coverage](./testing.md)** - Comprehensive test suite documentation

## Market Operations

- **[Market Overview](./contract/market/index.md)** - Core market functionality
- **[Supply Assets](./contract/market/supply.md)** - How to supply assets to earn yield
- **[Borrow Assets](./contract/market/borrow.md)** - How to borrow against collateral
- **[Liquidations](./contract/market/liquidate.md)** - Liquidation mechanisms and procedures

## Additional Resources

- **[Implementation Notes](./notes.md)** - Technical implementation details
- **[API Reference](./api-reference.md)** - The backend HTTP API, generated Rust API documentation, and the gateway method catalog
- **[Live risk dashboard](https://data.templarfi.org/)** - Real-time coverage and liquidation analytics across markets
- **[Source code](https://github.com/Templar-Protocol/contracts)** - Contracts, services, and tooling
