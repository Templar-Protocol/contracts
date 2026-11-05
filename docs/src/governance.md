# Protocol Governance

This document outlines the administrative structure and governance controls of Templar Protocol: what can and cannot be changed, by whom, and with what delay.

## Summary

| Component | Mutability | Controlled by | Timelock |
|---|---|---|---|
| Market contracts (NEAR) | No admin functions, no upgrade method, no pause. Storage can be patched only while the deployer's full-access key is retained (see below). | Nobody through the contract; the Templar DAO multisig (2-of-3) while a deployer key is retained | n/a |
| Proxy oracles | Feed configuration and code upgradeable through a dedicated governance contract. Until the deployer's full-access key is deleted, the key holder can also change the account directly (see below). | Either the vault curators or the Templar DAO multisig (2-of-3), per proxy oracle; retained deployer keys are also held by the DAO multisig | 24h to 168h by action; emergency trips immediate; not enforced against a retained deployer key |
| Registry | Owner approves versions, manages roles and upgrades registry code. Uploader stages code; Deployer creates contracts. Cannot touch deployed markets. | Current Owner (intended: Templar DAO multisig, 2-of-3); explicitly granted hot-key roles | Two-step ownership transfer; global versions finalize after a probe, not a timelock |
| Curated vaults (Stellar) | Governed; runtime upgradeable through the vault governance contract | Vault governance admin, curator, and Sentinel per vault | Configurable per action; risk-reducing actions immediate |

## Administrative Multisig

Mutable Templar contracts on NEAR are by default administered by [`templar.sputnik-dao.near`](https://nearblocks.io/address/templar.sputnik-dao.near), a [Sputnik DAO](https://github.com/near-daos/sputnik-dao-contract) (v2) whose sole council role holds three signers at a **2-of-3** threshold. Administrative actions are executed as DAO function-call proposals against the target contract; no signer can act alone.

Changing the signer set is itself a governed policy change: a council member submits a `ChangePolicy` (or `AddMemberToRole` / `RemoveMemberFromRole`) proposal, the remaining members vote, and the DAO applies the change to itself when the threshold is reached. Signer additions and removals are announced on the official channels.

### DAO-approved MPC signing

Some administrative actions are not function calls a DAO can make directly — adding or removing an account's access keys, deploying code, or a transaction that must originate from the account itself. For those, an operational account can hold a full-access key derived by the NEAR Chain Signatures contract (`v1.signer`) for the DAO. The key never leaves the MPC network: to use it, a council member proposes a `sign` call carrying the hash of the exact payload (a NEP-366 delegate action or a transaction), voters review the payload (carried in the proposal, or shared alongside it when it should not be public before execution) against that hash, and once the proposal executes the MPC returns a signature that anyone can relay. The DAO threshold therefore governs these actions exactly as it governs direct calls, with two additional dependencies: the MPC network's availability and its key derivation, both audited by NEAR One. The network signs only when 11 of its 16 operators cooperate; see [Chain Signatures MPC Network](./security-overview.md#chain-signatures-mpc-network). Which accounts hold a derived key is part of the deployment records.

## Market Contracts

Market contracts have **no administrative functions**:

- They operate autonomously based on their initial configuration, which is immutable after deployment.
- There is no method to pause, upgrade, or modify market parameters.
- There is no privileged access to user funds.

At the account level, the registry adds each full-access key explicitly supplied in a deployment's `full_access_keys` to each new market account. The parameter can be omitted and the registry does not add the deploying signer's key automatically. While such a key is retained, its holder can modify contract storage through the reviewed, sandbox-replayed patch process in [Deploying a market](./deployments.md#patching-contract-storage); this is how legacy markets were migrated to proxy oracles. A market account with no access keys cannot be changed by anyone. Verify every child account's actual keys with:

```bash
near account list-keys <market-address> network-config mainnet now
```

### Deployer Key Retention

When the registry deploys a market (or a proxy oracle and its governance contract), it installs only the full-access keys supplied through `full_access_keys`. `tmplrmgr` includes its DAO key by default and lists every key each account will receive in the deployment plan, but neither the registry nor `full_access_keys` requires that choice or a DAO threshold. In the current deployment, retained keys are held by [`templar.sputnik-dao.near`](https://nearblocks.io/address/templar.sputnik-dao.near), whose 2-of-3 council threshold prevents one signer from acting alone; [DAO-approved MPC signing](#dao-approved-mpc-signing) is how that DAO exercises a full-access key. Keys held elsewhere remain ordinary NEAR full-access keys controlled by their holders: they can deploy code, patch storage, or change keys directly. A retained DAO key supports the reviewed [patch process](./deployments.md#patching-contract-storage) and was used in August 2026 to migrate legacy markets to proxy oracles. Verify every child account's keys before applying the DAO-key policy; the patch and sign-off process is operational policy, not a control enforced by the target account.

**Policy**: retained deployer keys are deleted after a bake-in period. Apply this to every key actually present on the market and, where one is deployed alongside it, its proxy oracle and governance contract; do not assume that only the DAO key was installed. The bake-in period for a market ends once the curators allocating to it and the issuers of its assets have signed off on the deployment. The DAO then makes one key-removal call per retained key and contract account; each removal is reported in the [public alerts channel](https://t.me/+CcqXyt01lsljZmQx). After all keys are deleted, a market becomes immutable at the account level as well as at the contract level: no storage patch is possible and any fix requires a new market version and user migration. A proxy oracle or governance contract can still be changed, but only through the governance contract's timelocked proposals.

Anyone can check whether an account still carries an access key with the `near account list-keys` command above; an empty list means the key has been deleted.

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

The [registry](./contract/registry.md) is owned. On role-enabled V2 registries,
Admin is the **current Owner**, not a grantable role. Owner bypasses action role
guards and alone can approve versions by hash (`approve_version`), remove stored
version code, upgrade the registry, and call `set_role` for Uploader or Deployer.
Legacy `add_version`, including `PublishGlobal`, remains Owner-only during caller
migration. Standard Owner methods retain two-step propose/accept transfer and
`own_renounce_owner`; Owner does **not** prevent renunciation. Owner plus RBAC
preserves existing `own_*` clients and ownership storage. An RBAC Admin-only
model is viable but does not justify an otherwise unnecessary API/storage cutover.

Uploader pre-funds public NEP-145 storage accounting with `storage_deposit`,
then stages Borsh bytes through `upload_code` with zero attached deposit.
Removing its own pending upload requires one yocto and credits the original
payer's available balance; Owner can remove any pending upload. Uploaded approval
keeps the payer charged for retained code until `remove_version`. Freed pending
storage is likewise credited to its payer. Users withdraw available balances
with `storage_withdraw`; these are user liabilities, not a nonrefundable fee or
an Owner treasury. There is no new administrative withdrawal method.
Normal registry-funded writes also retain capacity for outstanding deployment
callbacks and standard Owner-slot growth. V2 upgrades check backing after code
deployment and migration within the same atomic receipt, not only before dispatch.

Deployer can create contracts from approved versions and supply `full_access_keys`.
Those keys can control the children, including replacing code; the role must be
trusted for that authority, not only deployment from approved bytes. Losing
Uploader also prevents removal of earlier uploads. Explicit grants survive
ownership transfer; Owner's action bypass moves immediately to the new Owner.
`has_role` and `list_role` report explicit membership only, not Owner bypass.
`set_role` requires one yocto, defaults `set` to true and
`allow_removing_last_member` to false, and emits
`role_set {account_id, role, is_granted}` under `templar-registry` version `1.0.0`
on every successful call, including idempotence. Older registries retain
Owner-only registration/deployment; do not assume the V2 rollout is complete.

The implementation-owned deployment labels `probe` and `deploy` are reserved even
for Owner and Deployer: neither role can occupy the scratch accounts required for
global publication and approval. Before upgrading or publishing, operators must
verify that no existing child at either label blocks those operations; the
reservation does not remove an already-existing child.

The intended owner is the DAO multisig; verify live `own_get_owner`, roles and
keys rather than assuming rollout has occurred. The [canary-to-mainnet
procedure](./deployments.md#role-enabled-v2-rollout-alpha-then-v1) requires DAO
approval/deployment and same-code upgrade proofs before registry key deletion.

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
