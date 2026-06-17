#!/usr/bin/env node
// Inspect a base64 or hex Pyth Pro Solana-format payload and verify its Ed25519 signature.
//
// Usage:
//   node contract/pyth-pro/solana-payload-signer.mjs <BASE64_OR_HEX_PAYLOAD>
//   cat <captured.solana.base64> | node contract/pyth-pro/solana-payload-signer.mjs

import crypto from "node:crypto";
import fs from "node:fs";
import process from "node:process";

const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const SOLANA_FORMAT_MAGIC = 2182742457;
const PAYLOAD_FORMAT_MAGIC = 2479346549;
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

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

function readInput() {
  const arg = process.argv[2];
  if (arg) {
    if (fs.existsSync(arg) && fs.statSync(arg).isFile()) {
      return fs.readFileSync(arg, "utf8").trim();
    }
    return arg.trim();
  }

  if (process.stdin.isTTY) {
    throw new Error("Pass a payload string, a payload file path, or pipe payload data on stdin");
  }
  return fs.readFileSync(0, "utf8").trim();
}

function decodePayload(rawInput) {
  const input = rawInput.startsWith("0x") ? rawInput.slice(2) : rawInput;
  if (/^[0-9a-fA-F]+$/.test(input) && input.length % 2 === 0) {
    return Buffer.from(input, "hex");
  }
  return Buffer.from(input, "base64");
}

function parseFeedIds(payload) {
  let offset = 0;
  const magic = payload.readUInt32LE(offset);
  offset += 4;
  const timestampUs = payload.readBigUInt64LE(offset);
  offset += 8;
  const channel = payload.readUInt8(offset);
  offset += 1;
  const feedCount = payload.readUInt8(offset);
  offset += 1;

  const feedIds = [];
  for (let i = 0; i < feedCount; i += 1) {
    const feedId = payload.readUInt32LE(offset);
    offset += 4;
    feedIds.push(feedId);
    const propertyCount = payload.readUInt8(offset);
    offset += 1;

    for (let j = 0; j < propertyCount; j += 1) {
      const property = payload.readUInt8(offset);
      offset += 1;
      // Property tags = `PriceFeedProperty` discriminants (0-indexed) in the pinned protocol fork.
      switch (property) {
        case 0:  // price
        case 1:  // bestBidPrice
        case 2:  // bestAskPrice
        case 5:  // confidence
        case 10: // emaPrice
        case 11: // emaConfidence
          offset += 8; // bare LE i64 (0 = absent)
          break;
        case 3:  // publisherCount (u16)
        case 4:  // exponent (i16)
        case 9:  // marketSession (i16)
          offset += 2;
          break;
        case 6:  // fundingRate
        case 7:  // fundingTimestamp
        case 8:  // fundingRateInterval
        case 12: { // feedUpdateTimestamp
          const present = payload.readUInt8(offset); // 1-byte presence flag + optional LE i64/u64
          offset += 1;
          if (present) offset += 8;
          break;
        }
        default:
          throw new Error(`Unsupported property ${property} in payload parser`);
      }
    }
  }

  if (offset !== payload.length) {
    throw new Error(`Payload parser left ${payload.length - offset} trailing bytes`);
  }
  return { magic, timestampUs: timestampUs.toString(), channel, feedIds };
}

const raw = decodePayload(readInput());
if (raw.length < 102) throw new Error(`Solana payload too short: ${raw.length}`);

const envelopeMagic = raw.readUInt32LE(0);
const signature = raw.subarray(4, 68);
const publicKey = raw.subarray(68, 100);
const payloadLength = raw.readUInt16LE(100);
const payload = raw.subarray(102);

if (envelopeMagic !== SOLANA_FORMAT_MAGIC) {
  throw new Error(`Unexpected Solana envelope magic: ${envelopeMagic}`);
}
if (payloadLength !== payload.length) {
  throw new Error(`Payload length mismatch: header=${payloadLength}, actual=${payload.length}`);
}

const key = crypto.createPublicKey({
  key: Buffer.concat([ED25519_SPKI_PREFIX, publicKey]),
  format: "der",
  type: "spki",
});
const signatureVerified = crypto.verify(null, payload, key, signature);
const parsedPayload = parseFeedIds(payload);
if (parsedPayload.magic !== PAYLOAD_FORMAT_MAGIC) {
  throw new Error(`Unexpected inner payload magic: ${parsedPayload.magic}`);
}

console.log(
  JSON.stringify(
    {
      publicKey: base58Encode(publicKey),
      publicKeyHex: publicKey.toString("hex"),
      signatureVerified,
      payloadLength,
      timestampUs: parsedPayload.timestampUs,
      channel: parsedPayload.channel,
      feedIds: parsedPayload.feedIds,
    },
    null,
    2,
  ),
);
