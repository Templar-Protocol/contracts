import { jest, describe, expect, test } from "@jest/globals";

/**
 * Mock the SDK BEFORE importing the module that uses it.
 * This prevents any real network calls during the test.
 */
jest.mock("@redstone-finance/sdk", () => {
  return {
    // Return a deterministic hex payload that `handle` expects.
    requestRedstonePayload: jest.fn(async () => "deadbeef"),
    // Provide a simple stable value for signers helper.
    getSignersForDataServiceId: jest.fn(() => []),
  };
});

import handle from "./handle";
import { parseArgs } from "./args";
import { Request } from "./msg";
import { requestRedstonePayload } from "@redstone-finance/sdk";

describe("handle requests", () => {
  test("request", async () => {
    const args = parseArgs(
      "--data-service-id redstone-primary-prod".split(" "),
    );
    const req: Request = {
      id: 1059,
      method: "fetch",
      params: ["ETH", "BTC", "CETES", "USTRY"],
    };

    const res = await handle(args, req);

    expect(res.id).toBe(1059);
    expect(res.status).toBe("success");
    if (res.status !== "success") {
      return;
    }

    // Our mock returns 'deadbeef' which is 4 bytes when interpreted as hex.
    const bytes = Buffer.from(res.data, "hex");
    expect(bytes.length).toBeGreaterThan(0);

    // Ensure the mocked SDK function was called.
    expect(
      (requestRedstonePayload as unknown as jest.Mock).mock.calls.length,
    ).toBeGreaterThan(0);
  });
});
