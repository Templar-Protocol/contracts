# Deployment specs

One TOML file per deployed market, grouped by registry:

- `alpha/` — markets under `templar-alpha.near`
- `v1/` — markets under `v1.tmplr.near`
- `profiles/` — fragments markets share through `extends`

A market states only what is specific to it; everything shared comes from the
profiles it extends, in order, with the market itself winning over all of them.

## Asset naming

A market is named `<collateral>-<borrow>`, and each side is built from three
parts:

1. `i` if the token is held on the `intents.near` contract.
2. The host chain, when the asset is not native to it — `eth` in `iethwbtc`,
   `xlm` in `ixlmusdc`.
3. The asset's symbol.

So `iethwbtc` is WBTC on Ethereum held through Intents, `ixlmusdc` is USDC on
Stellar held through Intents, and `ibtc` is native BTC held through Intents.

The distinction matters: v1 borrows three different tokens all called USDC —
`ixlmusdc`, `iethusdc`, and native `usdc.near` — so a profile or market named
for the bare ticker would be ambiguous.

## Profiles

| Profile | Supplies |
|---|---|
| `alpha-mainnet` / `v1-mainnet` | registry, versions, governance, shared market params |
| `irs-standard` / `irs-stable` | one interest-rate curve |
| `v1-borrow-*` / `v1-collateral-*` | one asset leg: token, decimals, aggregator, sources, and the `symbol`/`reference` pair the price cross-check needs |

Asset profiles carry `symbol` and `reference` because neither reaches the
chain: `market export` cannot recover them, so a spec reconstructed from a
deployed market has neither and its `reference.price.*` checks cannot run.

Each asset profile is checked against the markets it represents — see
`a_shared_asset_profile_matches_the_markets_it_represents` in
`tools/manager/src/tests/spec.rs`. Adopting one must change nothing deployed.
