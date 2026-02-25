import { describe, expect, test } from "@jest/globals";
import { Args, parseArgs } from "./args.ts";

describe("argument parsing", () => {
  test("empty default", () => {
    const expected: Args = {
      socket: "/tmp/templar_redstone_bridge.sock",
      "data-service-id": "redstone-primary-demo",
      "unique-signers-count": 3,
      "wait-for-all-gateways-time-ms": 1000,
      "max-timestamp-deviation-ms": 60 * 1000,
    };

    expect(parseArgs([])).toEqual(expected);
  });

  test("socket", () => {
    const expected: Args = {
      socket: "/path/to/custom.sock",
      "data-service-id": "redstone-primary-demo",
      "unique-signers-count": 3,
      "wait-for-all-gateways-time-ms": 1000,
      "max-timestamp-deviation-ms": 60 * 1000,
    };

    expect(parseArgs(["--socket", "/path/to/custom.sock"])).toEqual(expected);
  });

  test("authorized signers", () => {
    const expected: Args = {
      socket: "/tmp/templar_redstone_bridge.sock",
      "data-service-id": "redstone-primary-demo",
      "unique-signers-count": 3,
      "wait-for-all-gateways-time-ms": 1000,
      "max-timestamp-deviation-ms": 60 * 1000,
      "authorized-signers": "redstone-fast-demo",
    };

    expect(parseArgs(["--authorized-signers", "redstone-fast-demo"])).toEqual(
      expected,
    );
  });

  test("unique signers count", () => {
    const expected: Args = {
      socket: "/tmp/templar_redstone_bridge.sock",
      "data-service-id": "redstone-primary-demo",
      "unique-signers-count": 300,
      "wait-for-all-gateways-time-ms": 1000,
      "max-timestamp-deviation-ms": 60 * 1000,
    };

    expect(parseArgs(["--unique-signers-count", "300"])).toEqual(expected);
  });

  test("all options", () => {
    const expected: Args = {
      socket: "mysocket",
      "data-service-id": "redstone-main-demo",
      "unique-signers-count": 100,
      "wait-for-all-gateways-time-ms": 888,
      "max-timestamp-deviation-ms": 2,
      "authorized-signers": "redstone-megaeth-testnet",
    };

    expect(
      parseArgs(
        "--socket mysocket --data-service-id redstone-main-demo --unique-signers-count 100 --wait-for-all-gateways-time-ms 888 --max-timestamp-deviation-ms 2 --authorized-signers redstone-megaeth-testnet".split(
          " ",
        ),
      ),
    ).toEqual(expected);
  });

  test("odd number", () => {
    expect(() =>
      parseArgs("--socket mysocket --data-service-id".split(" ")),
    ).toThrow();
  });

  test("unknown option", () => {
    expect(() => parseArgs("--weird foo".split(" "))).toThrow();
  });

  test("invalid value: number", () => {
    expect(() => parseArgs("--unique-signers-count true".split(" "))).toThrow();
  });

  test("invalid value: data service id", () => {
    expect(() => parseArgs("--data-service-id 1".split(" "))).toThrow();
  });
});
