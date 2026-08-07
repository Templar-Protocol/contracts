# Deploying a market

A market is described by one spec file and deployed in two commands. There is
no shell script: the spec is the source of truth, and everything that used to
live in `env.sh`, `market-args.json` and `proxy-*.json` is derived from it.

`deployments/alpha` targets a mainnet registry, so both commands need
`--network mainnet` — the CLI defaults to testnet. `NETWORK` and `SIGNER_ID`
work in place of the flags.

```sh
tmplrmgr market plan  deployments/alpha/<market>.toml --out plan.json \
    --network mainnet --signer-id "$REGISTRY_OWNER" --public-key ed25519:…
tmplrmgr market apply --plan plan.json \
    --network mainnet --signer-id "$REGISTRY_OWNER" --sign-with keychain
```

The signer is not a personal account: `registry.deploy` asserts the registry's
owner, and a proxy spec additionally requires it to equal `governance.admin`,
which the mainnet profiles set to the registry itself.

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
extends = ["../profiles/alpha.toml", "../profiles/irs-standard.toml"]
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

### Amounts

Every amount states its unit, and the tool does the scaling:

```toml
[market]
borrow_range            = { minimum = "1 atom" }
supply_range            = { minimum = "0.04 tokens" }
supply_withdrawal_range = { minimum = "0.04 tokens", maximum = "1000 tokens" }
origination_fee         = { Flat = "0 atoms" }
```

`tokens` counts whole units of the borrow asset, scaled by its `decimals` when
the plan is built — `"0.04 tokens"` is four cents of a stablecoin whether it
carries 6 decimals or 7. `atoms` counts the indivisible base units the chain
stores, so `"1 atom"` says "no real floor" in a way `"0.0000001 tokens"` does
not. All three ranges and both fees are denominated in the *borrow* asset;
nothing is stated in collateral.

The unit is mandatory. A bare number is refused rather than guessed at, as is a
`tokens` value with more decimal places than the asset can hold, or a fractional
`atom`. Both spellings parse (`1 atom`, `1 atoms`); the tool writes the plural.

This is what schema 5 changed. A schema 4 file wrote the same amounts as bare
base-unit integers, which are still well-formed numbers — read as whole units
they would be `10^decimals` too large — so such a file is refused by version and
must be re-authored, not renumbered.

## Checking before and after

```sh
tmplrmgr spec check   deployments/alpha/<market>.toml --network mainnet
tmplrmgr market verify <account-id> --network mainnet \
    --governance-admin <account-id> \
    --against deployments/alpha/<market>.toml
```

Both modes verify. A direct market reconstructs without proxies or governance,
so the two governance checks are skipped and everything else runs;
`--governance-admin` is still required and means nothing there.

`verify` re-runs the preflight against what is actually on chain and exits
non-zero on failure, so it can run on a schedule. That matters because the
governance call that configures a price feed is dispatched detached: it reports
success even when the oracle rejected the proxy, so deployed state is the only
witness that a market can price anything.

### Reading the report

Checks are printed to stderr as they run, grouped by what is being read, then
summarized. The summary leads with the failures, in full, and lists what was
skipped separately — a check that did not run proves nothing, and must never be
counted as one that passed.

```
→ registry versions
  ok   registry.version.market          v1.3.0
  FAIL registry.version.oracle          `0.5.9` is not registered in v1.tmplr.near; the depl…

5 check(s): 3 passed, 1 skipped, 1 FAILED

FAILED
  registry.version.oracle
    `0.5.9` is not registered in v1.tmplr.near; the deploy would fail partway
```

Colour is used only on a terminal, and `NO_COLOR` turns it off. `-q` silences
the report. stdout stays the machine-readable channel throughout, so
`spec check … >/dev/null` leaves the report alone and `… 2>/dev/null | jq`
leaves the JSON alone.

`--skip-check <id>` suppresses one verdict — every other check still runs, and
the report records what the skip suppressed, so an override stays reviewable
rather than reading as a pass. An id that matches no check is an error, since a
typo would otherwise silently suppress nothing. Available on `spec check`,
`market plan` and `market apply`.

## Resuming

`apply` journals each step beside the plan as it lands. If a run is
interrupted, re-running it skips what completed and continues from the first
incomplete step. A plan truncated to its completed prefix is refused rather than
reported complete: the re-derivation runs before the journal is consulted.
