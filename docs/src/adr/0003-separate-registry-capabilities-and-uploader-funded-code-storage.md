# 3. Separate registry capabilities and uploader-funded code storage

Date: 2026-11-06

## Status

Proposed

## Context

The registry previously concentrated version approval, upgrades, and deployment in its Owner and
accepted attached deposits for code registration. That made operational hot keys more powerful than
necessary and made the party ultimately responsible for retained-code storage unclear. ENG-830
requires separate upload and deployment capabilities, DAO hash-only approval, and retained legacy
registration during caller migration.

This decision covers registry authority, code-upload accounting, migration, and upgrade/deployment
storage safety. It does not change market contracts, gateway registration callers, or the legacy
`add_version`/`PublishGlobal` interfaces. It does not authorize a production upgrade, ownership
transfer, role grant, or key deletion.

Process exception: implementation PR #708 was already in progress before the ADR process was
introduced and remains unmerged. This standalone ADR is the acceptance gate before that
implementation merges; it is not retroactive approval or a claim of ADR-first development.

The design must preserve the standard Owner API and storage slots, conventional RBAC membership,
existing version/deployment history, and pending-upload Borsh payloads. In particular,
role membership is explicit while Owner authority is separate (`common/src/registry.rs:6-21`,
`contract/registry/src/lib.rs:186-232`). Migration must move collection handles without iterating
historical entries or changing owner slots (`contract/registry/src/state/migration.rs:13-34`).
Unless otherwise stated, source line references in this ADR refer to the
[implementation revision `b73acb7a`](https://github.com/Templar-Protocol/contracts/tree/b73acb7a519a49d4920ade35f22a58a043dcfb6a).

## Decision

Adopt one registry capability and storage model with these boundaries:

1. **Owner administers; Uploader stages; Deployer deploys.** Keep the standard `Owner` derive and
   two-step ownership transfer, including renunciation. Use conventional RBAC for explicit
   `Uploader` and `Deployer` membership. Through the contract API, only Owner changes roles,
   approves versions, removes versions, or upgrades the registry. Owner bypasses action guards
   without becoming a synthetic role member. Holders of any retained registry full-access keys
   remain trusted to deploy code directly, outside Owner and role guards, until those keys are
   deleted.
   `set_role` emits the conventional `role_set` event with `account_id`, `role`, and
   `is_granted` (`common/src/registry.rs:390-394`). A Deployer may attach explicitly supplied
   full-access keys to children it creates, so that role is a trusted deployment capability, not a
   guarantee of keyless children (`contract/registry/src/lib.rs:624-633,720-726`). The current
   deployment process installs a deployer key, confirms the child works, replaces it with a freshly
   derived MPC/DAO-controlled key, and has the DAO delete the deployer key to prove the new key
   works. Installing the MPC key directly would remove that confirmation step and still require
   full-access-key support, so restricting this capability to Owner would not eliminate it.
   `RegistryEvent::Deployed` version 1.1.0 adds `deployer` and `full_access_keys` to the successful
   finalization event for provenance; the `Deployment` view is unchanged. Implementation #708 must
   thread that metadata through finalization and test the exact event payload before merging.
2. **Pre-fund code storage through NEP-145.** Through the contract API, only Uploader or Owner may
   call `upload_code`; Deployer has no upload authority without a separate Uploader grant or Owner
   authority. It accepts Borsh bytes with zero attached deposit and charges actual map, key,
   metadata, and blob growth to the predecessor's available balance
   (`contract/registry/src/lib.rs:234-254`). Reject only duplicate pending uploads; approved
   aliases may reuse a hash without scanning or indexing historical versions. Approval moves the
   bytes to a version and transfers the retained charge to the original payer, including any net
   version-entry growth (`contract/registry/src/lib.rs:434-499`). Removing pending code or retained
   version code releases credit to that payer, who can withdraw it. There is no admin withdrawal or
   nonrefundable upload fee (`contract/registry/src/storage_management.rs:72-104,201-225`).
3. **Approve by hash while retaining legacy registration.** Owner-only `approve_version` accepts
   `Uploaded(hash)` or `Global(hash)` with one yoctoNEAR. Uploaded approval consumes the pending
   upload; global approval reuses the existing probe/finalize path. Successful version keys remain
   occupied, while failed probes release their provisional key and preserve a matching upload
   (`contract/registry/src/lib.rs:434-507,577-593`). Keep legacy `add_version`, its three source
   variants, permanent successful-key behavior, and callback compatibility.
4. **Migrate by moving handles, not history.** V2 adds uploads, per-version payer charges,
   aggregate available credit, deployment reserves, and measured storage overhead under enum
   storage keys (`contract/registry/src/state/mod.rs:14-26,47-74`). V1→V2 preserves existing map
   prefixes, historical bytes, and Owner slots, and initializes empty accounting maps. Historical
   approved code remains registry-funded. Same-V2 upgrades need empty migration arguments.
5. **Never spend withdrawable credit on protocol obligations.** Registry-funded metadata,
   unallocated Owner capacity, and final deployment records are reserved in the solvency check
   (`contract/registry/src/storage_management.rs:106-143`). Deployment dispatch reserves its final
   record through callback completion. Failed deployment and failed global cleanup release their
   reservations even when the registry is already underfunded
   (`contract/registry/src/lib.rs:577-593,747-789`). Upgrade deploy, migration, and the post-upgrade
   solvency check share one atomic receipt. A direct call may attach 300 Tgas. After DAO handoff,
   the upgrade action must use `GlobalHash` with already-published code and empty migration
   arguments: the tested DAO action has 280 Tgas beneath a 300-Tgas outer `act_proposal`, and the
   rollout must not put WASM in proposal input (`contract/registry/src/lib.rs:311-335`;
   `contract/registry/tests/roles.rs`).
   DAO proposal tooling must let operators override gas values, including a simple `--gas
   "250 Tgas"`-style option, rather than relying on an unadjustable default.
6. **Roll out only through verified DAO operation.** Use the documented canary-first procedure:
   drain legacy in-flight callbacks, verify the V1 migration and historical data, grant explicit
   roles, and transfer ownership through real DAO acceptance. Before deleting keys, prove DAO
   hash-only approval/deployment and same-code upgrade. Delete registry full-access keys last,
   then repeat the DAO same-code upgrade and a distinct Uploader→DAO approval→Deployer deployment
   with no registry signing key. Verify the remaining key list and those post-deletion receipts
   before promoting alpha to v1. No production action is part of this ADR.

## Alternatives

- **Keep Owner-only administration and attached upload deposits.** This preserves fewer interfaces,
  but leaves hot keys with approval/deployment authority and does not establish who funds code
  retained after approval. It also retains deposit/refund handling instead of available-credit
  accounting.
- **Require Owner for non-empty `full_access_keys`, or remove the parameter.** Rejected: this
  would break the established deployer-key to MPC/DAO-key rotation and its confirmation step while
  still leaving the deployment process dependent on full-access-key support.

- **Replace Owner with an RBAC Admin role or remove ownership.** That would centralize the same
  authority under another name while requiring owner-slot, API, and DAO-handoff migration. It adds
  risk without improving the required Uploader/Deployer separation.
- **Ban hashes already present in any historical version and build a hash index.** A global ban
  prevents legitimate aliases and independent retained charges. Migration-time or on-demand history
  scans also inherit proof-size and gas risks. Pending-only uniqueness avoids both the policy and
  the index.
- **Permit forced unregistration or an Owner storage withdrawal.** Those shortcuts can strand
  retained code or spend a payer's funds. They are incompatible with original-payer ownership of
  available credit.
- **Move every historical entry into a new storage layout during migration.** Rewriting entries
  would increase migration time, proof size, and failure surface without changing the semantics
  required by V2.

## Consequences

Hot-key authority is reduced to its required operation, and retained storage has an accountable
payer. DAO approvals no longer carry WASM through proposal inputs. The cost is explicit operational
funding and trust management: uploaders need enough available credit, approval can require a
payer top-up, the registry needs protocol-owned backing, and every supplied child key and Deployer
must be treated as security-sensitive.

`Deployed` now records the Deployer and supplied full-access keys for incident provenance; the
`Deployment` view remains unchanged. Storage accounting involves trusted operator parties and
small NEAR amounts. If a storage-credit shortfall needs reconciliation, a manual top-up or
adjustment is acceptable; solvency checks may block storage withdrawals until it is resolved.

The following checks are required before rollout:

- **Unit/integration:** exact role/event and VersionSource wire formats, including `Deployed` 1.1.0
  with Deployer/key provenance, duplicate-pending versus approved-alias behavior, payer
  charging/release, underpayment rollback, V1 history/Owner-slot preservation, and failed cleanup
  during a shortfall.
- **Sandbox/end-to-end:** the real authorization matrix, DAO-as-predecessor approval and keyless
  upgrade, released-V1 migration, in-flight callback compatibility, non-tail blob cleanup, and
  actual withdrawal to the original payer. The implementation evidence is 466 native tests and 84
  affected sandbox scenarios. DAO coverage begins at revision `b73acb7a` in
  [`roles.rs:241`](https://github.com/Templar-Protocol/contracts/blob/b73acb7a519a49d4920ade35f22a58a043dcfb6a/contract/registry/tests/roles.rs#L241);
  migration evidence is in
  [`migration.rs:139-198`](https://github.com/Templar-Protocol/contracts/blob/b73acb7a519a49d4920ade35f22a58a043dcfb6a/contract/registry/src/state/migration.rs#L139-L198);
  upgrade solvency coverage begins at
  [`solvency.rs:52`](https://github.com/Templar-Protocol/contracts/blob/b73acb7a519a49d4920ade35f22a58a043dcfb6a/contract/registry/tests/solvency.rs#L52).
- **Static checks:** affected all-target Clippy, registry Wasm compilation, formatting, generated
  documentation, and mdBook link checks.
- **Operational verification:** compare finalized code hashes, owner/roles, state versions, access
  keys, deployment records, and protocol/user balances at every rollout gate. The existing mock DAO
  proves DAO-as-predecessor behavior, not production council policy or Sputnik callback budgeting.
  Reproduce the 280-Tgas action beneath the outer `act_proposal` against actual Sputnik DAO v2
  before rollout. No testnet or mainnet execution is claimed by this record.

### Failure scenarios

| Scenario | Warning signs | Prevention and detection | Recovery |
|---|---|---|---|
| Protocol writes try to consume user credit | `Insufficient registry balance to cover storage liabilities`; reduced available balance | Reserve physical storage, Owner growth, and in-flight final records; compare liabilities before and after writes | Top up protocol-owned NEAR, then retry the rejected write; the failed receipt must not change credit or metadata |
| A failed deployment/probe would strand its key under an existing shortfall | Reserved deployment or provisional `GlobalHash` remains after callback failure | Cleanup releases storage and reservations without a new solvency assertion; query the entry and pending reserve | With the cleanup rule in place, the entry and reserve are released even though the intentional failure receipt still reports failure |
| An upgrade targets the wrong migration or lacks backing for local code | `needs_migration`, changed history/owner, failed post-upgrade check, or unexpected code hash | Validate the migration chain, drain legacy callbacks, and run deploy/migrate/check atomically; verify V2 data and exact code identity | The failed atomic receipt restores prior code/state. Correct the target or protocol funding, then retry after rechecking finalized state |

Compromised accounts follow the existing incident-response SOP. Stop new deposits to affected
children while that response proceeds; registry role revocation alone does not remove child keys.

Human acceptance of this ADR is a design decision only. Production execution remains separately
reviewed and authorized.
