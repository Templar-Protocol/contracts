#!/usr/bin/env node
/**
 * Derive Ethereum keypair from BIP39 seed phrase
 * 
 * Usage: node scripts/derive-eth-key.js "your twelve word seed phrase here"
 */

const bip39 = require('bip39');
const { hdkey } = require('ethereumjs-wallet');

// Get mnemonic from command line
const mnemonic = process.argv[2];

if (!mnemonic) {
    console.error('Usage: node derive-eth-key.js "your twelve word seed phrase here"');
    process.exit(1);
}

// Validate mnemonic
if (!bip39.validateMnemonic(mnemonic)) {
    console.error('Error: Invalid mnemonic phrase');
    process.exit(1);
}

// Derive seed from mnemonic
const seed = bip39.mnemonicToSeedSync(mnemonic);

// Derive Ethereum wallet using BIP44 path: m/44'/60'/0'/0/0
// 44' = BIP44
// 60' = Ethereum coin type
// 0' = Account 0
// 0 = External chain (not change)
// 0 = Address index 0
const hdWallet = hdkey.fromMasterSeed(seed);
const path = "m/44'/60'/0'/0/0";
const wallet = hdWallet.derivePath(path).getWallet();

// Get private key and address
const privateKey = wallet.getPrivateKey();
const address = wallet.getAddressString();

console.log('\n========================================');
console.log('Ethereum Wallet Derived from Mnemonic');
console.log('========================================\n');
console.log('Derivation Path:', path);
console.log('Address:        ', address);
console.log('\nPrivate Key (hex with 0x):');
console.log('0x' + privateKey.toString('hex'));
console.log('\nPrivate Key (hex without 0x):');
console.log(privateKey.toString('hex'));
console.log('\n⚠️  WARNING: Never share your private key or commit it to version control!\n');
