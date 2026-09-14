# Protocol Governance

This document outlines the administrative structure and governance controls of Templar Protocol: what can and cannot be changed, by whom, and with what delay.

## Summary

| Component | Mutability | Controlled by | Timelock |
|---|---|---|---|
| Market contracts (NEAR) | No admin functions, no upgrade method, no pause. Storage can be patched only while the deployer's full-access key is retained (see below). | Nobody through the contract; the DAO multisig while a deployer key is retained | n/a |
| Proxy oracles | Feed configuration and code upgradeable through a dedicated governance contract | Templar DAO multisig (2-of-3) and role holders | 24h to 168h by action; emergency trips immediate |
| Registry | Owned. Owner can register versions, deploy new contracts, and upgrade the registry's own code. Cannot touch deployed markets. | Templar DAO multisig (2-of-3) | Two-step finalize |
| Oracle adapters (Pyth Lazer, RedStone) | Owned; signer sets and configuration are admin-managed | Templar DAO multisig (2-of-3) | n/a |
| Curated vaults (Stellar) | Governed; runtime upgradeable through the vault governance contract | Vault governance admin, curator, and Sentinel per vault | Configurable per action; risk-reducing actions immediate |

## Administrative Multisig

All mutable Templar contracts on NEAR are administered by [`templar.sputnik-dao.near`](https://nearblocks.io/address/templar.sputnik-dao.near), a [Sputnik DAO](https://github.com/near-daos/sputnik-dao-contract) (v2) whose sole council role holds three signers at a **2-of-3** threshold. Administrative actions are executed as DAO function-call proposals against the target contract; no signer can act alone.

Changing the signer set is itself a governed policy change: a council member submits a `ChangePolicy` (or `AddMemberToRole` / `RemoveMemberFromRole`) proposal, the remaining members vote, and the DAO applies the change to itself when the threshold is reached. Signer additions and removals are announced on the official channels.

## Market Contracts

Market contracts have **no administrative functions**:

- They operate autonomously based on their initial configuration, which is immutable after deployment.
- There is no method to pause, upgrade, or modify market parameters.
- There is no privileged access to user funds.

At the account level, the registry adds the deployer's full-access key to each new market account. While that key is retained (by the DAO multisig), contract storage can be modified through the reviewed, sandbox-replayed patch process in [Deploying a market](./deployments.md#patching-contract-storage); this is how legacy markets were migrated to proxy oracles. A market account with no access keys cannot be changed by anyone. Check a market's keys with:

```bash
near account list-keys <market-address> network-config mainnet now
```

### Deployer Key Retention

When the registry deploys a market (or a proxy oracle and its governance contract), it adds the deploying signer's full-access key to the new account and logs a warning that it has done so; `tmplrmgr` includes that key by default and lists every key each account will receive in the deployment plan. The key exists so that contract storage can be repaired through the reviewed [patch process](./deployments.md#patching-contract-storage) while a market is new; it was used in August 2026 to migrate legacy markets to proxy oracles. The key is held by the DAO multisig, never by an individual, and a patch cannot be applied without a DAO proposal and a sandbox replay.

**Policy**: retained deployer keys are deleted after a bake-in period. The bake-in period for a market ends once the curators allocating to it and the issuers of its assets have signed off on the deployment; the DAO then removes the key, and the account becomes immutable at the account level as well as at the contract level. After deletion, no storage patch is possible and any fix requires a new market version and user migration. **Input needed**: where each key deletion is recorded (the [Deployment and Version Log](./release-log.md) is the natural place) and the current status per market.

Anyone can check whether a market still carries an access key with the `near account list-keys` command above; an empty list means the key has been deleted.

New market versions are deployed to new account IDs through the registry; old versions cannot be overwritten. When a new version of the market contract is available, it is uploaded to the registry contract and new markets are deployed from it. Existing markets are not upgraded and funds are not automatically migrated, so users migrate their positions individually.

Because a market cannot be paused, the emergency control for a market that reads a proxy oracle is its oracle: tripping the [circuit breaker](./oracles.md#circuit-breakers) on the feed freezes borrowing, collateral withdrawal against debt, and liquidations on that market, while supply withdrawals and repayments continue. Older markets that read Pyth's contract directly have no such control.

## Proxy Oracle Governance

Each [proxy oracle](./oracles.md#proxy-oracle) is owned by its own governance contract (`proxy-gov-<market>.v1.tmplr.near`). Every change is a proposal that matures under a per-method timelock and can only be created and executed by an account holding the required role. The policy is set per governance contract; the table below is the typical production setup, not a guarantee for every contract:

| Action | Timelock | Required role |
|---|---|---|
| Trip or untrip a feed manually | none | `ManualTripper` |
| Re-arm a tripped breaker; enable or disable enforcement | none | `CircuitBreakerOperator` |
| Configure a feed's sources, weights, aggregator, freshness filter, or circuit breakers | 24 hours | `ProxyConfigurationManager` |
| Any other proxy oracle method (default) | 72 hours | `Admin` |
| Upgrade the proxy oracle contract | 72 hours | `Admin` |
| Change governance policy (timelocks, roles) | 48 hours | `Admin` |
| Upgrade the governance contract itself | 168 hours | `Admin` |

The immediate operator actions include their risk-increasing inverses (untripping a feed, disabling enforcement); those roles are held by the DAO multisig and every use is alerted. Shortening a timelock must itself mature under the timelock being shortened, so the policy cannot be weakened faster than it currently protects. The `Admin` role is held by the DAO multisig. Always confirm the policy in force on a specific proxy oracle with `get_governance_policy` on its governance contract, and pending proposals with `list_proposals`.

## Registry Contract

The [registry](./contract/registry.md) is an owned contract; its owner is the DAO multisig. The owner can:

- Register a new contract version (`add_version`, finalized in a second step) from an audited release.
- Remove a version so it can no longer be deployed.
- Deploy new markets and proxy oracles from registered versions.
- Upgrade the registry's own code.

The registry has no authority over contracts it has already deployed: it cannot modify, pause, or upgrade a live market. Registering a version does not change any existing deployment.

## Curated Vault Governance

Stellar vaults are governed by a per-vault governance contract with per-action timelocks, a curator who sets allocation policy, and an independent Sentinel that can pause and tighten restrictions immediately but cannot unpause or accept proposals. Risk-increasing changes (cap increases, new markets, fee increases, unpause, role changes, upgrades) are timelocked; risk-reducing changes execute immediately. See the [Stellar Vault Curator Guide](./curator-guide.md#governance-lifecycle) for the full rules.

## Emergency Procedures

Markets are immutable once deployed. If a bug is discovered:

1. Price-dependent operations on affected markets that read a proxy oracle can be frozen immediately by tripping the circuit breaker, and Templar's bots are halted.
2. A patched version of the code is audited, registered in the registry, and deployed to new market accounts.
3. Users migrate their funds individually. Supply withdrawal requests and repayments on the old market remain available throughout.

Contract code changes follow the full audit-and-multisig path even during an incident; there is no hotfix bypass. To facilitate migration swiftly and securely, users are encouraged to monitor all official communication channels and the [public alerts channel](https://t.me/+CcqXyt01lsljZmQx) for announcements. The full procedure is documented in the [emergency runbooks](https://github.com/Templar-Protocol/blend-contracts-v2/tree/main/docs/emergency-runbooks).

## Transparency and Monitoring

Templar contracts are open-source, with the source code available on [GitHub](https://github.com/Templar-Protocol/contracts). All completed audit reports are available in the [Templar audits folder](https://drive.google.com/drive/folders/14Q6iysMotto5fqpu6LRxjWBqeXElzkyF?usp=sharing) and summarized on the [Security](./security-overview.md#audits-and-formal-verification) page. Deployed contracts can be verified against the source; see [Contract Verification](./addresses.md#contract-verification). Governance events on proxy oracles, vaults, and the registry are monitored and alerted; see [Monitoring and Risk Management](./monitoring.md).
