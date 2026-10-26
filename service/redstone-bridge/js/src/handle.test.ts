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

    const res = await handle(args, "test-api-key", req);

    expect(res.id).toBe(1059);
    expect(res.status).toBe("success");
    if (res.status !== "success") {
      return;
    }

    // Our mock returns 'deadbeef' which is 4 bytes when interpreted as hex.
    const bytes = Buffer.from(res.data, "hex");
    expect(bytes.length).toBeGreaterThan(0);

    expect(requestRedstonePayload).toHaveBeenCalledWith(
      expect.objectContaining({
        dataServiceId: "redstone-primary-prod",
        dataPackagesIds: ["ETH", "BTC", "CETES", "USTRY"],
        authenticatedGateways: [{ apiKey: "test-api-key" }],
      }),
    );
  });

  test("failure flattens nested errors", async () => {
    (requestRedstonePayload as unknown as jest.Mock).mockImplementationOnce(
      async () => {
        throw new AggregateError(
          [new Error("Request failed with status code 403")],
          "requestDataPackages failed",
        );
      },
    );

    const res = await handle(parseArgs([]), "test-api-key", {
      id: 7,
      method: "fetch",
      params: ["BTC"],
    });

    expect(res).toEqual({
      id: 7,
      status: "failure",
      message:
        "requestDataPackages failed: Request failed with status code 403",
    });
  });
});
