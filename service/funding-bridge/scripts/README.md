# Funding Bridge Scripts

Utility scripts for managing and testing the Funding Bridge service.

## Prerequisites

### For derive-solana-key.js
Install Node.js dependencies (only needed once):
```bash
npm install bip39 ed25519-hd-key @solana/web3.js
```

### For service scripts
1. Build the project:
```bash
cargo build --release -p templar-funding-bridge
```

2. Create a `.env` file with required configuration (see `.env.example`)

## Scripts

### 1. derive-solana-key.js

Derive Solana keypair from a BIP39 seed phrase (mnemonic).

**Usage:**
```bash
node scripts/derive-solana-key.js "your twelve word seed phrase here"
```

**Example:**
```bash
node scripts/derive-solana-key.js "witch collapse practice feed shame open despair creek road again ice least"
```

**Output:**
- Public address (for receiving funds)
- Private key in Base58 format (recommended for `.env`)
- Private key in JSON array format (alternative)

**Security Warning:** Never share your private key or commit it to version control!

---

### 2. run_service.sh

Start the Funding Bridge service with proper configuration.

**Usage:**
```bash
./scripts/run_service.sh [mainnet|testnet]
```

**Examples:**
```bash
# Run with .env configuration (default)
./scripts/run_service.sh

# Run on mainnet (overrides NETWORK in .env)
./scripts/run_service.sh mainnet

# Run on testnet
./scripts/run_service.sh testnet
```

**Features:**
- Loads configuration from `.env` file
- Validates required environment variables
- Checks for running instances
- Displays service configuration before starting
- Supports network override via command line

**Required Environment Variables:**
- `NEAR_TREASURY_ACCOUNT` - Treasury account that holds OMFT balances and signs withdrawal intents
- `NEAR_TREASURY_KEY` - Private key for treasury account (ed25519:...)

**Optional Environment Variables:**
- `NETWORK` - NEAR network (mainnet)
- `DRY_RUN` - true or false (default: false)
- `RUST_LOG` - Logging level (default: info,templar_funding_bridge=debug)
- `ETH_PRIVATE_KEY` - Ethereum private key for EVM chain deposits
- `SOLANA_PRIVATE_KEY` - Solana private key (base58 format)
- `STELLAR_SECRET_KEY` - Stellar secret key (S... format)
- `NEAR_ACCOUNT` - External NEAR account for deposits/withdrawals
- `NEAR_KEY` - External NEAR private key for deposits/withdrawals

---

### 3. run_test.sh

Test deposit and withdrawal functionality of the running service.

**Usage:**
```bash
./scripts/run_test.sh <direction> <network> <amount>
```

**Arguments:**
- `direction` - `deposit` or `withdraw`
- `network` - `eth`, `arbitrum`, `base`, `optimism`, `solana`, `stellar`, `near`
- `amount` - Amount in USDC (e.g., 1.0 for 1 USDC)

**Examples:**
```bash
# Test withdrawal of 1 USDC to Solana
./scripts/run_test.sh withdraw solana 1.0

# Test withdrawal of 0.5 USDC to Ethereum
./scripts/run_test.sh withdraw eth 0.5

# Test withdrawal to NEAR
./scripts/run_test.sh withdraw near 2.0

# Test deposit from Solana
./scripts/run_test.sh deposit solana 10

# Test deposit from external NEAR wallet
./scripts/run_test.sh deposit near 5.0

# Test deposit from Stellar
./scripts/run_test.sh deposit stellar 2.0
```

**Note:** The service must be running before running tests. Start it with:
```bash
./scripts/run_service.sh
```

**Withdrawal Destinations:**
Configure in service `.env` file:
- `ETH_WITHDRAW_ADDRESS` - Ethereum
- `ARBITRUM_WITHDRAW_ADDRESS` - Arbitrum
- `BASE_WITHDRAW_ADDRESS` - Base
- `OPTIMISM_WITHDRAW_ADDRESS` - Optimism
- `POLYGON_WITHDRAW_ADDRESS` - Polygon
- `SOLANA_WITHDRAW_ADDRESS` - Solana
- `STELLAR_WITHDRAW_ADDRESS` - Stellar
- NEAR uses `NEAR_ACCOUNT` for withdrawals

---

## Quick Start

```bash
# 1. Install Node.js dependencies (one-time)
npm install

# 2. Generate Solana keypair (optional)
node scripts/derive-solana-key.js "your seed phrase"

# 3. Configure environment
cp .env.example .env
# Edit .env with your keys

# 4. Build service
cargo build --release -p templar-funding-bridge

# 5. Start service
./scripts/run_service.sh

# 6. Test withdrawals (in another terminal)
./scripts/run_test.sh withdraw solana 1.0
```

---

## Troubleshooting

**Service won't start:**
- Check binary exists: `ls -la ../../target/release/funding-bridge`
- Verify `.env` file has required variables
- Check port 3000 is available

**Tests fail with "Service not responding":**
- Start service: `./scripts/run_service.sh`
- Verify health: `curl http://localhost:3000/health`

**"Insufficient balance" error:**
- NEP-413 signing is working correctly
- Treasury needs USDC balance in NEAR Intents
- Deposit USDC before testing withdrawals

---

## Security Best Practices

- Never commit private keys (add `.env` to `.gitignore`)
- Use environment variables for keys
- Rotate keys regularly for production
- Test on testnet before using real funds
- Monitor logs for suspicious activity
