import assert = require("node:assert/strict");
import z = require("zod");

const DataServiceId = z.literal([
  "redstone-primary-demo",
  "redstone-main-demo",
  "redstone-avalanche-demo",
  "redstone-arbitrum-demo",
  "redstone-avalanche-prod",
  "redstone-primary-prod",
  "redstone-arbitrum-prod",
  "redstone-fast-demo",
  "redstone-megaeth-testnet",
  "redstone-perun-demo-1",
]);

const Args = z.object({
  ["socket"]: z.string().default("/tmp/templar_redstone_bridge.sock"),
  ["data-service-id"]: DataServiceId.default("redstone-primary-demo"),
  ["unique-signers-count"]: z.uint32().default(3),
  ["wait-for-all-gateways-time-ms"]: z.uint32().default(1000),
  ["max-timestamp-deviation-ms"]: z.uint32().default(60 * 1000),
  ["authorized-signers"]: DataServiceId.optional(),
});
export type Args = z.infer<typeof Args>;

export function parseArgs(argv: string[]): z.infer<typeof Args> {
  const argsObj = {} as any;

  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    assert.ok(key);
    assert.match(key, /^--\S+$/);
    const value = argv[i + 1];
    assert.ok(value, "missing value");
    argsObj[key.slice(2)] = value;
  }

  return Args.parse(argsObj);
}
