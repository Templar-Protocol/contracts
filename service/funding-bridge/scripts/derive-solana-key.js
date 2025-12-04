#!/usr/bin/env node
/**
 * Derive Solana keypair from BIP39 seed phrase
 *
 * This script derives a Solana keypair from a 12 or 24-word seed phrase
 * using the standard Phantom wallet derivation path (m/44'/501'/0'/0').
 *
 * Usage:
 *   node scripts/derive-solana-key.js "your twelve word seed phrase here"
 *
 * Example:
 *   node scripts/derive-solana-key.js "witch collapse practice feed shame open despair creek road again ice least"
 *
 * Output:
 *   - Public address (for receiving funds)
 *   - Private key in Base58 format (for SOLANA_KEYPAIR_BASE58 env var)
 *   - Private key in JSON array format (alternative format)
 *
 * Requirements:
 *   npm install bip39 ed25519-hd-key @solana/web3.js
 */

const bip39 = require('bip39');
const { derivePath } = require('ed25519-hd-key');
const { Keypair } = require('@solana/web3.js');

// Parse command line arguments
const seedPhrase = process.argv.slice(2).join(' ');

// Validate input
if (!seedPhrase) {
    console.error('❌ Error: No seed phrase provided\n');
    console.error('Usage: node scripts/derive-solana-key.js "your seed phrase"');
    console.error('Example: node scripts/derive-solana-key.js "witch collapse practice feed shame open despair creek road again ice least"');
    process.exit(1);
}

// Validate seed phrase format
if (!bip39.validateMnemonic(seedPhrase)) {
    console.error('❌ Error: Invalid seed phrase!');
    console.error('   Seed phrase must be 12 or 24 words from the BIP39 wordlist');
    process.exit(1);
}

try {
    // Derive seed from mnemonic
    const seed = bip39.mnemonicToSeedSync(seedPhrase);

    // Use Phantom/Solflare standard derivation path
    const derivationPath = "m/44'/501'/0'/0'";
    const derivedSeed = derivePath(derivationPath, seed.toString('hex')).key;

    // Create keypair from derived seed
    const keypair = Keypair.fromSeed(derivedSeed);

    // Convert secret key to Base58 manually (in case bs58 module issues)
    const alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
    function toBase58(bytes) {
        const digits = [0];
        for (let i = 0; i < bytes.length; i++) {
            let carry = bytes[i];
            for (let j = 0; j < digits.length; j++) {
                carry += digits[j] << 8;
                digits[j] = carry % 58;
                carry = (carry / 58) | 0;
            }
            while (carry > 0) {
                digits.push(carry % 58);
                carry = (carry / 58) | 0;
            }
        }
        return digits.reverse().map(d => alphabet[d]).join('');
    }

    // Output results
    console.log('═'.repeat(80));
    console.log('🔑  Solana Keypair Derived Successfully');
    console.log('═'.repeat(80));
    console.log('');
    console.log('📍 Public Address (for receiving funds):');
    console.log('   ' + keypair.publicKey.toBase58());
    console.log('');
    console.log('🔐 Private Key (Base58 format - recommended):');
    console.log('   ' + toBase58(keypair.secretKey));
    console.log('');
    console.log('📝 For .env file, add this line:');
    console.log('   SOLANA_KEYPAIR_BASE58=' + toBase58(keypair.secretKey));
    console.log('');
    console.log('─'.repeat(80));
    console.log('Alternative: Private Key (JSON Array format):');
    console.log('   ' + JSON.stringify(Array.from(keypair.secretKey)));
    console.log('');
    console.log('═'.repeat(80));
    console.log('⚠️  SECURITY WARNING: KEEP YOUR PRIVATE KEY SECRET!');
    console.log('   Never share it with anyone or commit it to version control');
    console.log('═'.repeat(80));

} catch (error) {
    console.error('❌ Error deriving keypair:', error.message);
    process.exit(1);
}
