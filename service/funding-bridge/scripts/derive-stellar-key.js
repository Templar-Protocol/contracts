#!/usr/bin/env node
/**
 * Derive Stellar keypair from BIP39 seed phrase
 *
 * This script derives a Stellar keypair from a 12 or 24-word seed phrase
 * using the standard Stellar derivation path (m/44'/148'/0').
 *
 * Usage:
 *   node scripts/derive-stellar-key.js "your twelve word seed phrase here"
 *
 * Example:
 *   node scripts/derive-stellar-key.js "witch collapse practice feed shame open despair creek road again ice least"
 *
 * Output:
 *   - Public address (for receiving funds)
 *   - Secret key (for STELLAR_SECRET_KEY env var)
 *
 * Requirements:
 *   npm install bip39 ed25519-hd-key @stellar/stellar-sdk
 */

const bip39 = require('bip39');
const { derivePath } = require('ed25519-hd-key');
const StellarSdk = require('@stellar/stellar-sdk');

// Parse command line arguments
const seedPhrase = process.argv.slice(2).join(' ');

// Validate input
if (!seedPhrase) {
    console.error('❌ Error: No seed phrase provided\n');
    console.error('Usage: node scripts/derive-stellar-key.js "your seed phrase"');
    console.error('Example: node scripts/derive-stellar-key.js "witch collapse practice feed shame open despair creek road again ice least"');
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

    // Use Stellar standard derivation path (account 0)
    // m/44'/148'/0' where 148 is Stellar's coin type
    const derivationPath = "m/44'/148'/0'";
    const derivedSeed = derivePath(derivationPath, seed.toString('hex')).key;

    // Create Stellar keypair from derived seed
    const keypair = StellarSdk.Keypair.fromRawEd25519Seed(Buffer.from(derivedSeed));

    // Output results
    console.log('═'.repeat(80));
    console.log('🔑  Stellar Keypair Derived Successfully');
    console.log('═'.repeat(80));
    console.log('');
    console.log('📍 Public Address (for receiving funds):');
    console.log('   ' + keypair.publicKey());
    console.log('');
    console.log('🔐 Secret Key (for signing transactions):');
    console.log('   ' + keypair.secret());
    console.log('');
    console.log('📝 For .env file, add this line:');
    console.log('   STELLAR_SECRET_KEY=' + keypair.secret());
    console.log('');
    console.log('📝 Withdraw address (same as public key):');
    console.log('   STELLAR_WITHDRAW_ADDRESS=' + keypair.publicKey());
    console.log('');
    console.log('═'.repeat(80));
    console.log('⚠️  SECURITY WARNING: KEEP YOUR SECRET KEY SECURE!');
    console.log('   Never share it with anyone or commit it to version control');
    console.log('═'.repeat(80));
    console.log('');
    console.log('💡 Additional accounts can be derived by modifying the path:');
    console.log('   Account 0: m/44\'/148\'/0\'  (default)');
    console.log('   Account 1: m/44\'/148\'/1\'');
    console.log('   Account 2: m/44\'/148\'/2\'');
    console.log('   etc.');

} catch (error) {
    console.error('❌ Error deriving keypair:', error.message);
    process.exit(1);
}
