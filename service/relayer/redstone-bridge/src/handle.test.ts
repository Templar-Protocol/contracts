import { describe, expect, test } from "@jest/globals";
import handle from "./handle";
import { parseArgs } from "./args";
import { Request } from "./msg";

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
    const bytes = Buffer.from(res.data, "hex");
    expect(bytes.length).toBeGreaterThan(0);
  });
});
