# Deployed configurations, as they were at migration

The `market-args.json` each alpha market was deployed with, kept as the
regression corpus for ENG-548: every spec under `tools/manager/specs/alpha/`
must still reproduce the configuration recorded here.

This is **test data, not a deployment path**. The files that made it one —
`script/deploy.sh`, and each market's `env.sh` and `proxy-*.json` — were
deleted with the migration; git history has them if that shape is ever needed.
