# Deployed configurations, as they were at migration

One `<market>.<registry>.json` per market — the init args it was deployed with —
kept as the regression corpus for ENG-548: every configuration recorded here
must still be reproduced by its spec under `deployments/`. The converse does not
hold; a spec whose market is not yet on chain has nothing to record and is
covered by no test.

Two tests pin the size of this corpus, so retiring a market means deleting its
recording *and* decrementing both: the `#[case]`s on
`migrated_specs_reproduce_their_deployed_configurations`
(`tools/manager/src/tests/spec.rs`) and the total in
`contract/market/src/tests.rs::parse_configurations`. Delete the market's
subdirectory too, if it has one — nothing counts those files.

Proxy-mode deployments also keep their `proxy-collateral.json` /
`proxy-borrow.json` in such a subdirectory. Only the two alpha markets named in
`src/tests/export.rs` and `derived_proxies_match_the_deployed_proxy_files` are
read; the rest are kept as a record of what was deployed.

This is **test data, not a deployment path**. The files that made it one —
`script/deploy.sh` and each market's `env.sh` — were deleted with the migration;
git history has them if that shape is ever needed.
