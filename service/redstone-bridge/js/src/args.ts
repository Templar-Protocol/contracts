import assert from "node:assert/strict";
import z from "zod";

// Mirrors the data services in the SDK's bundled oracle registry.
const DataServiceId = z.literal([
  "redstone-primary-demo",
  "redstone-primary-prod",
  "redstone-fast-demo",
  "redstone-megaeth-demo",
  "redstone-megaeth-prod",
  "redstone-perun-demo-1",
  "redstone-hip3-demo",
  "redstone-hip3-prod",
]);

const stringToInt = z.codec(z.string().regex(z.regexes.integer), z.int(), {
  decode: (str) => Number.parseInt(str, 10),
  encode: (num) => num.toString(),
});

export const Args = z
  .object({
    ["socket"]: z.string().default("/tmp/templar_redstone_bridge.sock"),
    ["data-service-id"]: DataServiceId.default("redstone-primary-demo"),
    ["unique-signers-count"]: stringToInt.default(3),
    ["wait-for-all-gateways-time-ms"]: stringToInt.default(1000),
    ["max-timestamp-deviation-ms"]: stringToInt.default(60 * 1000),
    ["authorized-signers"]: DataServiceId.optional(),
  })
  .strict();
export type Args = z.infer<typeof Args>;

export function parseArgs(argv: string[]): Args {
  const argsObj = {} as any;

  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    assert.ok(key);
    assert.match(key, /^--\S+$/);
    const value = argv[i + 1];
    assert.ok(value, "missing value");
    argsObj[key.slice(2)] = value;
  }

  return Args.decode(argsObj);
}

const API_KEY_ENV = "REDSTONE_API_KEY";

export function parseApiKey(env: NodeJS.ProcessEnv): string {
  return z
    .string(`${API_KEY_ENV} must be set`)
    .trim()
    .min(1, `${API_KEY_ENV} must not be empty`)
    .parse(env[API_KEY_ENV]);
}
