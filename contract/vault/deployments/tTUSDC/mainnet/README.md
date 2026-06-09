# tTUSDC Mainnet Deployment

This directory stores the canonical deployment manifest for the mainnet test
Templar USDC vault stack.

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
