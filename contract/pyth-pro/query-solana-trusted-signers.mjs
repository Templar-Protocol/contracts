#!/usr/bin/env node
// Query and decode the Pyth Pro Solana storage account without Anchor.
//
// Usage:
//   node contract/pyth-pro/query-solana-trusted-signers.mjs
//   SOLANA_RPC_URL=https://api.mainnet-beta.solana.com node contract/pyth-pro/query-solana-trusted-signers.mjs

import process from "node:process";

const RPC_URL =
  process.env.PYTH_PRO_SOLANA_RPC_URL ??
  process.env.SOLANA_RPC_URL ??
  "https://api.mainnet-beta.solana.com";
const PROGRAM_ID =
  process.env.PYTH_PRO_SOLANA_PROGRAM ??
  "pytd2yyk641x7ak7mkaasSJVXh6YYZnC7wTmtgAyxPt";
const STORAGE_ACCOUNT =
  process.env.PYTH_PRO_SOLANA_STORAGE ??
  "3rdJbqfnagQ4yx9HXJViD4zc4xpiSqmFsKpPuSCQVyQL";

const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const ANCHOR_DISCRIMINATOR_BYTES = 8;
const PUBKEY_BYTES = 32;
const U64_BYTES = 8;
const I64_BYTES = 8;
const SPACE_FOR_TRUSTED_SIGNERS = 5;
const EXPECTED_ACCOUNT_BYTES = 381;

function base58Encode(bytes) {
  let value = BigInt(`0x${Buffer.from(bytes).toString("hex") || "0"}`);
  let out = "";
  while (value > 0n) {
    const mod = Number(value % 58n);
    out = BASE58_ALPHABET[mod] + out;
    value /= 58n;
  }
  for (const byte of bytes) {
    if (byte === 0) out = `1${out}`;
    else break;
  }
  return out || "1";
}

function unixSecondsToIso(seconds) {
  if (seconds <= 0n) return null;
  return new Date(Number(seconds) * 1000).toISOString();
}

function readPubkey(data, offset) {
  const bytes = data.subarray(offset, offset + PUBKEY_BYTES);
  return {
    base58: base58Encode(bytes),
    hex: bytes.toString("hex"),
  };
}

function readTrustedSigner(data, offset, nowS) {
  const pubkey = readPubkey(data, offset);
  const expiresAt = data.readBigInt64LE(offset + PUBKEY_BYTES);
  return {
    publicKey: pubkey.base58,
    publicKeyHex: pubkey.hex,
    expiresAt: expiresAt.toString(),
    expiresAtIso: unixSecondsToIso(expiresAt),
    active: expiresAt > nowS,
  };
}

async function getStorageAccount() {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 15_000);
  const response = await fetch(RPC_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    signal: controller.signal,
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getAccountInfo",
      params: [STORAGE_ACCOUNT, { encoding: "base64", commitment: "confirmed" }],
    }),
  }).finally(() => clearTimeout(timeout));
  if (!response.ok) {
    throw new Error(`Solana RPC failed: ${response.status} ${response.statusText}`);
  }
  const body = await response.json();
  if (body.error) {
    throw new Error(`Solana RPC error: ${JSON.stringify(body.error)}`);
  }
  const account = body.result?.value;
  if (!account) {
    throw new Error(`Storage account not found: ${STORAGE_ACCOUNT}`);
  }
  return {
    context: body.result.context,
    account,
    data: Buffer.from(account.data[0], "base64"),
  };
}

function decodeStorage(data, account, context) {
  if (data.length !== EXPECTED_ACCOUNT_BYTES) {
    throw new Error(`Unexpected storage account size: got ${data.length}, expected ${EXPECTED_ACCOUNT_BYTES}`);
  }
  if (account.owner !== PROGRAM_ID) {
    throw new Error(`Unexpected storage account owner: got ${account.owner}, expected ${PROGRAM_ID}`);
  }

  let offset = ANCHOR_DISCRIMINATOR_BYTES;
  const topAuthority = readPubkey(data, offset);
  offset += PUBKEY_BYTES;
  const treasury = readPubkey(data, offset);
  offset += PUBKEY_BYTES;
  const singleUpdateFeeLamports = data.readBigUInt64LE(offset);
  offset += U64_BYTES;

  const numTrustedSigners = data.readUInt8(offset);
  offset += 1;
  if (numTrustedSigners > SPACE_FOR_TRUSTED_SIGNERS) {
    throw new Error(`Invalid numTrustedSigners: ${numTrustedSigners}`);
  }
  const trustedSignersBase = offset;
  const nowS = BigInt(Math.floor(Date.now() / 1000));
  const trustedSigners = [];
  for (let i = 0; i < numTrustedSigners; i += 1) {
    trustedSigners.push(
      readTrustedSigner(data, trustedSignersBase + i * (PUBKEY_BYTES + I64_BYTES), nowS),
    );
  }

  return {
    rpcUrl: RPC_URL,
    slot: context.slot,
    programId: PROGRAM_ID,
    storageAccount: STORAGE_ACCOUNT,
    topAuthority: topAuthority.base58,
    treasury: treasury.base58,
    singleUpdateFeeLamports: singleUpdateFeeLamports.toString(),
    trustedSigners,
  };
}

const { context, account, data } = await getStorageAccount();
console.log(JSON.stringify(decodeStorage(data, account, context), null, 2));
