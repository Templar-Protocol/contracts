import { describe, expect, test } from "@jest/globals";
import { Request } from "./msg";

describe("message serialization", () => {
  test("can deserialize Rust request", () => {
    const rustMessage = JSON.parse(
      `{"id":123,"method":"fetch","params":["ETH","BTC"]}`,
    );
    Request.parse(rustMessage);
    const parsed = Request.parse(rustMessage);
    expect(parsed.id).toBe(123);
    expect(parsed.method).toBe("fetch");
    expect(parsed.params).toEqual(["ETH", "BTC"]);
  });
});
