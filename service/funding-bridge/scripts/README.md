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
- `NEAR_ACCOUNT` - NEAR account that signs intents
- `NEAR_SIGNER_KEY` - Private key for NEAR account (ed25519:...)

**Optional Environment Variables:**
- `NETWORK` - mainnet or testnet (default: mainnet)
- `DRY_RUN` - true or false (default: false)
- `RUST_LOG` - Logging level (default: info,templar_funding_bridge=debug)
- `SOLANA_PRIVATE_KEY` - Solana private key for deposits (base58 format)
- `ETH_PRIVATE_KEY` - Ethereum private key for deposits

---

### 3. run_test.sh

Test deposit and withdrawal functionality of the running service.

**Usage:**
```bash
./scripts/run_test.sh <direction> <network> <amount>
```

**Arguments:**
- `direction` - `deposit` or `withdraw`
- `network` - `eth` (Ethereum) or `solana`
- `amount` - Amount in USDC (e.g., 1.0 for 1 USDC)

**Examples:**
```bash
# Test withdrawal of 1 USDC to Solana
./scripts/run_test.sh withdraw solana 1.0

# Test withdrawal of 0.5 USDC to Ethereum
./scripts/run_test.sh withdraw eth 0.5

# Deposit testing info
./scripts/run_test.sh deposit solana 10
```

**Note:** The service must be running before running tests. Start it with:
```bash
./scripts/run_service.sh
```

**Withdrawal Destinations:**
Withdrawal destination addresses are configured in the service `.env` file:
- `ETH_WITHDRAW_ADDRESS` - Ethereum address for withdrawals
- `ARBITRUM_WITHDRAW_ADDRESS` - Arbitrum address for withdrawals
- `BASE_WITHDRAW_ADDRESS` - Base address for withdrawals
- `OPTIMISM_WITHDRAW_ADDRESS` - Optimism address for withdrawals
- `POLYGON_WITHDRAW_ADDRESS` - Polygon address for withdrawals
- `SOLANA_WITHDRAW_ADDRESS` - Solana address for withdrawals

---

## Quick Start

1. **Install Node.js dependencies (one-time setup):**
   ```bash
   npm install
   ```

2. **Generate Solana keypair from seed phrase:**
   ```bash
   node scripts/derive-solana-key.js "your seed phrase here"
   ```

3. **Configure environment:**
   ```bash
   cp .env.example .env
   # Edit .env with your keys
   ```

4. **Build the service:**
   ```bash
   cargo build --release -p templar-funding-bridge
   ```

5. **Start the service:**
   ```bash
   ./scripts/run_service.sh
   ```

6. **Test withdrawals (in another terminal):**
   ```bash
   ./scripts/run_test.sh withdraw solana 1.0
   ```

---

## Troubleshooting

### Service won't start
- Check that the binary exists: `ls -la ../../target/release/funding-bridge`
- Verify `.env` file exists and has required variables
- Check for port conflicts (default port is 3000)

### Tests fail with "Service not responding"
- Make sure the service is running: `./scripts/run_service.sh`
- Check service is listening: `curl http://localhost:3000/health`

### "insufficient balance" error
- This means NEP-413 signing is working correctly
- The treasury account needs USDC balance in NEAR Intents
- Deposit USDC to NEAR Intents first before testing withdrawals

---

## Security Best Practices

1. **Never commit private keys** - Add `.env` to `.gitignore`
2. **Use environment variables** - Don't hardcode keys in scripts
3. **Rotate keys regularly** - Especially for production accounts
4. **Test on testnet first** - Before using real funds
5. **Monitor logs** - Check for suspicious activity

---

## Development

To modify scripts:

1. **Make changes** - Edit scripts in `scripts/` directory
2. **Make executable** - `chmod +x scripts/*.sh`
3. **Test** - Run scripts with test configuration
4. **Document** - Update this README with changes
