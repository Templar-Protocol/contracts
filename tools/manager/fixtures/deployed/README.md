# Deployed configurations, as they were at migration

The `market-args.json` each market was deployed with — 18 under `alpha/`, 27
under `v1/` — kept as the regression corpus for ENG-548: every spec under
`deployments/` must still reproduce the configuration recorded here.

Proxy-mode deployments also keep their `proxy-collateral.json` /
`proxy-borrow.json` in a subdirectory, so the derived oracle configuration is
checked against the deployed one too.

This is **test data, not a deployment path**. The files that made it one —
`script/deploy.sh` and each market's `env.sh` — were deleted with the migration;
git history has them if that shape is ever needed.
