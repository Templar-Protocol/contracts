# Deploying a market

A market is described by one spec file and deployed in two commands. There is
no shell script: the spec is the source of truth, and everything that used to
live in `env.sh`, `market-args.json` and `proxy-*.json` is derived from it.

```
tmplrmgr market plan  tools/manager/specs/alpha/<market>.toml --out plan.json \
    --signer-id <you> --public-key ed25519:…
$EDITOR plan.json          # optional
tmplrmgr market apply --plan plan.json --sign-with keychain
```

`plan` reads the chain and writes a file; it sends nothing and takes no
credential. `apply` sends what the file says.

## Why two steps

The plan is a reviewable artifact. It lists every transaction, decodes the
market configuration for reading, names the keys each new account will grant,
and carries the results of every preflight check. Reviewing a deployment no
longer means reading a shell script and trusting it matches four JSON files.

It is also editable, for when the spec cannot express something. `apply`
re-derives what it can from the steps that will actually execute — the accounts
being created, the oracle each step references, the admin being seated — so an
edit that makes the deployment incoherent is refused by name rather than sent.

## Writing a spec

Shared values live in `tools/manager/specs/alpha/profiles/`. A market file names the profiles
it extends and states only what differs:

```toml
extends = ["profiles/alpha-mainnet.toml", "profiles/irs-standard.toml"]
name = "my-market"

[oracle.direct]                    # reads an oracle that already exists
account_id = "pyth-oracle.near"

[collateral]
asset = "nep141:usdc.near"
price_id = "eaa020c6…"             # the oracle's own identifier
decimals = 6
```

Omit `[oracle.direct]` to deploy a dedicated proxy oracle instead. A proxy
market names `sources` per asset and the deployment creates a governance
contract, the oracle it owns, and the market — seven transactions rather than
one.

## Checking before and after

```
tmplrmgr spec check   tools/manager/specs/alpha/<market>.toml       # before deploying
tmplrmgr market verify <account-id> --governance-admin <account-id> \
    --against tools/manager/specs/alpha/<market>.toml               # after
```

`market verify` currently reconstructs a spec by reading back a proxy oracle
this tool deployed, so it works for proxy markets only — the 16 direct markets
are covered by `spec check`, which validates the oracle they read and the
price identifiers it serves. Extending verify to direct markets is tracked
separately.

`verify` re-runs the preflight against what is actually on chain and exits
non-zero on failure, so it can run on a schedule. That matters because the
governance call that configures a price feed is dispatched detached: it reports
success even when the oracle rejected the proxy, so deployed state is the only
witness that a market can price anything.

## Resuming

`apply` journals each step beside the plan as it lands. If a run is
interrupted, re-running it skips what completed and continues from the first
incomplete step. Editing a step that has already run is refused; editing one
that has not is allowed, which is the point.
