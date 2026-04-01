# Proxy Oracle Operations Runbook

Examples use [the `tmplrmgr` CLI](https://github.com/Templar-Protocol/contracts/tree/dev/tools/manager).

Price identifiers in the CLI are hex-encoded 32-byte values.

## Concepts

A proxy oracle maps each market-facing `price_id` to a `Proxy` definition.

A `Proxy` definition includes:

- an aggregation method
- filter settings such as `max_age`, `max_clock_drift`, and `min_sources`
- one or more weighted entries
- entries that point either to direct oracle requests or transformer-based requests

Governance actions are applied through proposals rather than direct mutation.

Transformers are especially useful for LSTs: the underlying asset price comes from the oracle, the redemption rate comes from chain state, and the proxy derives the LST price.

## Governance Lifecycle

For any feed change:

1. Inspect the current feed definition.
2. Build the replacement proxy JSON.
3. Create a governance proposal.
4. Review the proposal on-chain.
5. Wait for the configured TTL.
6. Execute the proposal.
7. Re-check the live proxy definition.

Cancel instead of executing if the change should not proceed.

## Inspect The Current State

List proxied feeds:

```bash
tmplrmgr proxy-oracle proxy list \
  --oracle-id <proxy_oracle_account>
```

Get one feed:

```bash
tmplrmgr proxy-oracle proxy get \
  --oracle-id <proxy_oracle_account> \
  --price-id <price_identifier>
```

Get one feed as JSON:

```bash
tmplrmgr proxy-oracle proxy get \
  --oracle-id <proxy_oracle_account> \
  --price-id <price_identifier> \
  --json
```

List proposals:

```bash
tmplrmgr proxy-oracle governance list \
  --oracle-id <proxy_oracle_account>
```

Get one proposal:

```bash
tmplrmgr proxy-oracle governance get \
  --oracle-id <proxy_oracle_account> \
  --id <proposal_id> \
  --json
```

## Deploy A Proxy Oracle

The deploy command has two modes:

- `direct`: deploys the contract directly onto an existing account
- `from-registry`: asks a registry contract to deploy a new proxy oracle account

### Direct Deploy

Deploy directly to an existing account:

```bash
tmplrmgr proxy-oracle deploy \
  direct \
  --signer-id <proxy_oracle_account> \
  --secret-key <secret_key>
```

Useful flags:

- `--no-build` to skip rebuilding and use an existing WASM artifact
- `--workspace-path <path>` to load the contract from a different workspace root

### Deploy From Registry

Deploy through a registry:

```bash
tmplrmgr proxy-oracle deploy \
  from-registry \
  --registry-id <registry_account> \
  --version-key <proxy_oracle_version> \
  --name <new_proxy_oracle_name> \
  --signer-id <registry_signer> \
  --secret-key <secret_key>
```

This creates `<name>.<registry_id>`.

Useful flags:

- `--deposit <amount>` to override the default deployment deposit
- `--with-full-access-key <public_key>` to add additional full-access keys
- `--no-signer-full-access-key` to avoid adding the signer's key to the deployed account

## Proxy Definition Patterns

### Single-Source Feed

Use for direct passthrough.

```json
{
  "aggregator": {
    "method": "MedianLow",
    "filter": {
      "max_age": "60000000000",
      "max_clock_drift": "10000000000",
      "min_sources": 1
    }
  },
  "entries": [
    {
      "source": {
        "Request": {
          "Pyth": {
            "oracle_id": "pyth-oracle.near",
            "price_id": "<underlying_price_id>"
          }
        }
      },
      "weight": 1
    }
  ]
}
```

### Primary Plus Backup Feed

Use for a preferred source plus backups.

```json
{
  "aggregator": {
    "method": "Priority",
    "filter": {
      "max_age": "60000000000",
      "max_clock_drift": "10000000000",
      "min_sources": 1
    }
  },
  "entries": [
    {
      "source": {
        "Request": {
          "Pyth": {
            "oracle_id": "pyth-oracle.near",
            "price_id": "<pyth_price_id>"
          }
        }
      },
      "weight": 1
    },
    {
      "source": {
        "Request": {
          "RedStone": {
            "oracle_id": "<redstone_adapter_account>",
            "price_id": "<redstone_feed_id>"
          }
        }
      },
      "weight": 1
    }
  ]
}
```

### Priority Feed

Use when the highest-priority live source should win.

```json
{
  "aggregator": {
    "method": "Priority",
    "filter": {
      "max_age": "60000000000",
      "max_clock_drift": "10000000000",
      "min_sources": 1
    }
  },
  "entries": [
    {
      "source": {
        "Request": {
          "Pyth": {
            "oracle_id": "pyth-oracle.near",
            "price_id": "<pyth_price_id>"
          }
        }
      },
      "weight": 10
    },
    {
      "source": {
        "Request": {
          "RedStone": {
            "oracle_id": "<redstone_adapter_account>",
            "price_id": "<redstone_feed_id>"
          }
        }
      },
      "weight": 5
    }
  ]
}
```

## Add Or Update A Feed

Create or update a feed:

```bash
tmplrmgr proxy-oracle governance create \
  --signer-id <governance_signer> \
  --secret-key <secret_key> \
  --oracle-id <proxy_oracle_account> \
  proxy \
  --price-id <price_identifier> \
  --insert '<proxy_json>'
```

Notes:

- use `--insert-file <path>` instead of `--insert` for file input
- omit `--id` to auto-fetch the next proposal ID
- add `--execute-immediately` only if the created proposal TTL is zero

Review the proposal:

```bash
tmplrmgr proxy-oracle governance list \
  --oracle-id <proxy_oracle_account>

tmplrmgr proxy-oracle governance get \
  --oracle-id <proxy_oracle_account> \
  --id <proposal_id> \
  --json
```

Execute after the TTL elapses:

```bash
tmplrmgr proxy-oracle governance execute \
  --signer-id <governance_signer> \
  --secret-key <secret_key> \
  --oracle-id <proxy_oracle_account> \
  --id <proposal_id>
```

Verify the live feed:

```bash
tmplrmgr proxy-oracle proxy get \
  --oracle-id <proxy_oracle_account> \
  --price-id <price_identifier> \
  --json
```

## Create A Backup Oracle Path

Use this when adding a production backup source to an existing feed.

1. Read the current feed definition.
2. Add the backup source as a second `entries` item.
3. Keep the same market-facing `price_id`.
4. Choose the aggregation method:
   - use `MedianLow` if you want live-source combination
   - use `Priority` if you want a preferred source to dominate while backups remain available
5. Create the governance proposal.
6. Wait for TTL and execute.
7. Re-check the live definition.

Markets do not need to be reconfigured.

## Remove A Broken Source

Replace the feed definition with a full JSON payload that omits the broken source.

Treat the `Proxy` definition as complete desired state, not a partial patch.

## Remove A Feed Entirely

Remove the mapping:

```bash
tmplrmgr proxy-oracle governance create \
  --signer-id <governance_signer> \
  --secret-key <secret_key> \
  --oracle-id <proxy_oracle_account> \
  proxy \
  --price-id <price_identifier> \
  --remove
```

This creates a `SetProxy` operation with `proxy = None`.

## Change Governance TTL

Set the delay for future proposals:

```bash
tmplrmgr proxy-oracle governance create \
  --signer-id <governance_signer> \
  --secret-key <secret_key> \
  --oracle-id <proxy_oracle_account> \
  set-ttl \
  --secs 3600
```

You can also use `--ms` or `--ns`.

Notes:

- the new TTL applies to future proposals
- existing proposals keep the TTL snapshot recorded when they were created

## Cancel A Proposal

```bash
tmplrmgr proxy-oracle governance cancel \
  --signer-id <governance_signer> \
  --secret-key <secret_key> \
  --oracle-id <proxy_oracle_account> \
  --id <proposal_id>
```

Use this for superseded or unwanted proposals.

## Verification Checklist

Before execution, confirm:

- the target `price_id`
- every upstream `oracle_id`
- feed identifiers for each source
- the aggregation method matches intent
- `min_sources` matches the liveness requirement
- freshness and clock-drift windows
- the proposal ID and execution order

After execution:

- re-read the live proxy definition
- confirm the governance queue no longer shows the executed proposal
- confirm off-chain services that depend on oracle resolution have the expected inputs
- confirm monitoring covers every upstream dependency in the new definition

## Operating Notes

- proposals execute in queue order; an earlier active proposal must be executed or cancelled first
- empty proxy definitions are rejected by contract validation
- proxy proposal creation supports `--insert`, `--insert-file`, and `--remove`
- proposal and proxy inspection commands support `--json`
- the proxy oracle supports backup-source composition without changing market configuration
- transformer-based entries can be used to derive prices for assets such as LSTs from an underlying oracle price plus an on-chain redemption rate
- off-chain services may resolve the underlying source requests directly, so configuration changes should be documented and communicated as part of normal change management
