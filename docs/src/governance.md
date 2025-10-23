# Protocol Governance

This document outlines the current administrative structure and governance controls of Templar Protocol.

## Registry Contract

The registry contract is immutable once deployed and locked. It designates an owner account, which has permission to add new contract code versions and to deploy new contracts. There is no upgrade mechanism.

## Market Contracts

Market contracts are immutable once deployed and locked. The configuration is immutable after deployment. New market versions can be deployed to new account IDs via a registry, but old versions cannot be overwritten. There is no upgrade mechanism. When a new version of the market contract is available, it will be uploaded to the registry contract. New markets can then be deployed using the updated code. However, old markets will not be upgraded, and funds will not be automatically migrated, so users will need to migrate their positions individually.

Market contracts have **no administrative functions**:

- Operate autonomously based on initial configuration.
- No ability to pause, upgrade, or modify market parameters.

## Emergency Procedures

Markets are immutable once they are deployed. If a bug is discovered, a patched version of the code will be uploaded and affected markets will have new versions deployed. However, users will need to migrate their funds individually. To facilitate this process swiftly and securely, users are encouraged to monitor all official communication channels for announcements.

## Transparency and Monitoring

Templar markets are open-source, and the source code currently available on [GitHub](https://github.com/Templar-Protocol/contracts). All completed audits will be made available as soon as possible. See [the current list of audits](https://github.com/Templar-Protocol/contracts/tree/dev/audits).
