import { describe, expect, test } from "@jest/globals";
import { Request, Response } from "./msg";

describe("message serialization", () => {
  test("can deserialize Rust request", () => {
    const rustMessage = JSON.parse(
      `{"id":123,"method":"fetch","params":["ETH","BTC"]}`,
    );
    Request.parse(rustMessage);
  });
});
