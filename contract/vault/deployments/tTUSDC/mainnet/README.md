# tTUSDC Mainnet Deployment

This directory stores the canonical deployment manifest for the mainnet test
Templar USDC vault stack.

This is an experimental mainnet test deployment for "Test Templar USDC"
(`tTUSDC`), not a production vault. The recorded governance constructor uses
`timelock_ns = 0`, equivalent to deploying with `SOROBAN_GOV_TIMELOCK_NS=0` and
`SOROBAN_ALLOW_ZERO_GOV_TIMELOCK=1`. A zero timelock removes the normal delay for
reviewing governance proposals before execution, so this configuration can put
user funds at immediate risk and must remain temporary/test-only.

Use this state path for CLI commands:

```sh
--state /data/tmp/contracts-soroban-vault-cli/contract/vault/deployments/tTUSDC/mainnet/manifest.json
```

The previous recovery path is a symlink to this manifest:

```text
/data/tmp/mainnet-vault-recovery-manifest.json
```

Gateway RPC completed the final resume cleanly after `mainnet.sorobanrpc.com`
timed out after submission:

```sh
--rpc-url https://soroban-rpc.mainnet.stellar.gateway.fm
```
