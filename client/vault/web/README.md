# @templar/vault-web

TypeScript web client for Templar Vault. Handles deposit, withdraw, and refresh flows with Wallet Selector integration.

## Installation

```bash
npm install @templar/vault-web
```

## Development

### Generate ABI & Types

The package uses TypeScript types generated from the vault contract ABI. To regenerate after contract changes:

```bash
# From contracts/client/vault/
make abi
```

This will:
1. Generate the contract ABI via `cargo near abi`
2. Copy `vault.abi.json` to `web/src/abi/generated/`
3. Generate TypeScript types from the ABI schema

To regenerate types only (without rebuilding the ABI):

```bash
# From contracts/client/vault/web/
npm run generate-types
```

### Build

```bash
npm run build
```

### Test

```bash
npm run test
```

### Typecheck

```bash
npm run typecheck
```
