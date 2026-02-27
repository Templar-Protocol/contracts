import { describe, test } from "@jest/globals";
import { Request } from "./msg";

describe("message serialization", () => {
  test("can deserialize Rust request", () => {
    const rustMessage = JSON.parse(
      `{"id":123,"method":"fetch","params":["ETH","BTC"]}`,
    );
    Request.parse(rustMessage);
  });
});
