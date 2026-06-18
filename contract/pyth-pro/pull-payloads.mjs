#!/usr/bin/env node
// Throwaway helper for capturing Pyth Pro / Lazer payload fixtures over the WS stream.
//
// Usage:
//   npm install --no-save ws
//   PYTH_PRO_API_KEY=... FEED_IDS=7,8,1,27,23 node contract/pyth-pro/pull-payloads.mjs
//
// Optional: PYTH_PRO_CHANNEL=fixed_rate@200ms  PYTH_PRO_OUT=payloads  MAX_MESSAGES=5

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import WebSocket from "ws";

const TOKEN = process.env.PYTH_PRO_API_KEY ?? process.env.ACCESS_TOKEN;
if (!TOKEN) {
  console.error("Set PYTH_PRO_API_KEY or ACCESS_TOKEN.");
  process.exit(1);
}

const FEED_IDS = (process.env.FEED_IDS ?? "")
  .split(",")
  .map((s) => Number(s.trim()))
  .filter((n) => Number.isInteger(n) && n > 0);
if (FEED_IDS.length === 0) {
  console.error(
    "Set FEED_IDS to a comma-separated list of Lazer feed ids, e.g. FEED_IDS=7,8,1,27,23.",
  );
  process.exit(1);
}

const WS_URL =
  process.env.PYTH_PRO_WS_URL ?? "wss://pyth-lazer-0.dourolabs.app/v1/stream";
const CHANNEL = process.env.PYTH_PRO_CHANNEL ?? "fixed_rate@200ms";
const OUT_DIR = process.env.PYTH_PRO_OUT ?? "payloads";
const MAX_MESSAGES = Number.parseInt(process.env.MAX_MESSAGES ?? "5", 10);
if (!Number.isInteger(MAX_MESSAGES) || MAX_MESSAGES <= 0) {
  console.error("MAX_MESSAGES must be a positive integer.");
  process.exit(1);
}
const FIELD_NAME = "solana";
const PROPERTIES = [
  "price",
  "confidence",
  "exponent",
  "emaPrice",
  "emaConfidence",
  "feedUpdateTimestamp",
];

function writePayloadFiles(index, message) {
  const prefix = path.join(
    OUT_DIR,
    `${String(index).padStart(3, "0")}-${message.parsed?.timestampUs ?? index}`,
  );
  fs.writeFileSync(`${prefix}.json`, `${JSON.stringify(message, null, 2)}\n`);

  const binary = message[FIELD_NAME];
  if (!binary?.data) {
    throw new Error(
      `streamUpdated message did not include the requested ${FIELD_NAME} payload`,
    );
  }
  const encoding = binary.encoding ?? "hex";
  const other = encoding === "hex" ? "base64" : "hex";
  fs.writeFileSync(`${prefix}.${FIELD_NAME}.${encoding}`, `${binary.data}\n`);
  fs.writeFileSync(
    `${prefix}.${FIELD_NAME}.${other}`,
    `${Buffer.from(binary.data, encoding).toString(other)}\n`,
  );
}

fs.mkdirSync(OUT_DIR, { recursive: true });

const ws = new WebSocket(WS_URL, {
  headers: { Authorization: `Bearer ${TOKEN}` },
});
let count = 0;

ws.on("open", () => {
  console.log(`Connected to ${WS_URL}`);
  ws.send(
    JSON.stringify({
      type: "subscribe",
      subscriptionId: 1,
      priceFeedIds: FEED_IDS,
      properties: PROPERTIES,
      formats: [FIELD_NAME],
      channel: CHANNEL,
      ignoreInvalidFeeds: true,
    }),
  );
});

ws.on("message", (data) => {
  let message;
  try {
    message = JSON.parse(data.toString());
  } catch (error) {
    console.error("Skipping non-JSON WS frame:", error);
    return;
  }
  if (message.type !== "streamUpdated") {
    console.log(JSON.stringify(message, null, 2));
    return;
  }
  if (count >= MAX_MESSAGES) return;
  count += 1;
  try {
    writePayloadFiles(count, message);
  } catch (error) {
    console.error("Failed to write payload files:", error);
    ws.close();
    process.exitCode = 1;
    return;
  }
  console.log(`Wrote payload ${count}/${MAX_MESSAGES}`);
  if (count >= MAX_MESSAGES) ws.close();
});

ws.on("close", () => console.log(`Done. Output: ${OUT_DIR}`));
ws.on("error", (error) => {
  console.error(error);
  process.exitCode = 1;
});
