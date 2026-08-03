# Deploying a market

A market is described by one spec file and deployed in two commands. There is
no shell script: the spec is the source of truth, and everything that used to
live in `env.sh`, `market-args.json` and `proxy-*.json` is derived from it.

`deployments/alpha` targets a mainnet registry, so both commands need
`--network mainnet` — the CLI defaults to testnet. `NETWORK` and `SIGNER_ID`
work in place of the flags.

```sh
tmplrmgr market plan  deployments/alpha/<market>.toml --out plan.json \
    --network mainnet --signer-id <you> --public-key ed25519:…
tmplrmgr market apply --plan plan.json \
    --network mainnet --signer-id <you> --sign-with keychain
```

`plan` reads the chain and writes a file; it sends nothing and takes no
credential. `apply` sends what the file says.

## Why two steps

The plan is a reviewable artifact. It lists every transaction, decodes the
market configuration for reading, names the keys each new account will grant,
and carries the results of every preflight check. Reviewing a deployment no
longer means reading a shell script and trusting it matches four JSON files.

It is a record of a derivation, not an input: the file carries the spec it came
from, and `apply` re-derives the steps and refuses anything that does not match.
Editing the plan is therefore not a way to change a deployment — change the spec
and re-plan. For something no spec can express, run the transaction yourself
with the command that performs it (`registry deploy`, `proxy-oracle governance
create-proposal`, `storage deposit`); each is typed and validated on its own.

## Writing a spec

Shared values live in `deployments/profiles/`. A market file names the profiles
it extends and states only what differs. Abbreviated — a market also needs a
`[borrow]` leg and the `[market]` parameters the profiles above do not set; see
any file under `deployments/` for a complete one:

```toml
extends = ["../profiles/alpha-mainnet.toml", "../profiles/irs-standard.toml"]
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
contract, the oracle it owns, and the market — seven transactions, plus one
storage registration per NEP-141 asset, rather than one.

## Checking before and after

```sh
tmplrmgr spec check   deployments/alpha/<market>.toml       # before deploying
tmplrmgr market verify <account-id> --governance-admin <account-id> \
    --against deployments/alpha/<market>.toml               # after
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
incomplete step. A plan truncated to its completed prefix is refused rather than
reported complete: the re-derivation runs before the journal is consulted.
