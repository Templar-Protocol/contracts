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

The design must preserve the standard Owner API and storage slots, conventional RBAC membership,
existing version/deployment history, callback ABIs, and pending-upload Borsh payloads. In particular,
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
   guarantee of keyless children (`contract/registry/src/lib.rs:624-633,720-726`).
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
   solvency check share one atomic receipt; attach 300 Tgas so code decoding and deployment retain
   enough gas (`contract/registry/src/lib.rs:311-335`).
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

The following checks are required before rollout:

- **Unit/integration:** exact role/event and VersionSource wire formats, duplicate-pending versus
  approved-alias behavior, payer charging/release, underpayment rollback, V1 history/Owner-slot
  preservation, and failed cleanup during a shortfall.
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
  keys, deployment records, and protocol/user balances at every rollout gate. No testnet or mainnet
  execution is claimed by this record.

### Failure scenarios

| Scenario | Warning signs | Prevention and detection | Recovery |
|---|---|---|---|
| Protocol writes try to consume user credit | `Insufficient registry balance to cover storage liabilities`; reduced available balance | Reserve physical storage, Owner growth, and in-flight final records; compare liabilities before and after writes | Top up protocol-owned NEAR, then retry the rejected write; the failed receipt must not change credit or metadata |
| A failed deployment/probe would strand its key under an existing shortfall | Reserved deployment or provisional `GlobalHash` remains after callback failure | Cleanup releases storage and reservations without a new solvency assertion; query the entry and pending reserve | With the cleanup rule in place, the entry and reserve are released even though the intentional failure receipt still reports failure |
| An upgrade targets the wrong migration or lacks backing for local code | `needs_migration`, changed history/owner, failed post-upgrade check, or unexpected code hash | Validate the migration chain, drain legacy callbacks, and run deploy/migrate/check atomically; verify V2 data and exact code identity | The failed atomic receipt restores prior code/state. Correct the target or protocol funding, then retry after rechecking finalized state |
| A compromised Deployer attaches an unintended child key | Unexpected child full-access key, code change, storage patch, or additional unauthorized keys | Treat Deployer as trusted deployment authority; review `full_access_keys` and enumerate actual child keys after deployment | Follow the compromised-Deployer recovery procedure below |

### Compromised-Deployer recovery

1. Determine whether the compromised Deployer is also Owner. If so, secure or transfer Owner
   authority before any child-key removal; revoking Deployer alone does not contain Owner's
   bypass. If Owner cannot be secured or transferred, stop using affected children, disclose the
   compromise, and migrate users where feasible instead of treating role revocation as containment.
2. After containing any compromised Owner authority, revoke the compromised Deployer through
   `set_role` before any child-key removal.
3. Wait for deployment receipts and callbacks authorized before revocation to settle, so no
   in-flight deployment can create another child after the role is revoked.
4. Re-enumerate child keys, including children created after the compromise. Remove every
   unauthorized key using a trusted full-access key on that child, if one remains.
5. Once the key list is clean, grant a reviewed replacement Deployer role if continued deployment
   is required. Rotating to a replacement does not revoke any child keys.
6. Verify the final key list and establish trusted child code and state before resuming use.
7. If integrity cannot be established or safely restored, disclose the compromise and migrate users
   to a newly reviewed market where feasible, even if a trusted child key remains. If no trusted
   child key or authorized removal method exists, stop using that Deployer and apply the same
   disclosure/migration fallback. A keyless market cannot be patched; recovery then requires a new
   market and user migration.

Human acceptance of this ADR is a design decision only. Production execution remains separately
reviewed and authorized.
