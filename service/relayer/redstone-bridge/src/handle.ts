import {
  getSignersForDataServiceId,
  requestRedstonePayload,
} from "@redstone-finance/sdk";
import type { Request, Response } from "./msg.js";
import type { Args } from "./args.js";

export default async function handle(
  args: Args,
  message: Request,
): Promise<Response> {
  try {
    switch (message.method) {
      case "fetch":
        console.debug("Fetching", message.params);

        const payloadString = await requestRedstonePayload({
          dataServiceId: args["data-service-id"],
          dataPackagesIds: message.params,
          uniqueSignersCount: args["unique-signers-count"],
          waitForAllGatewaysTimeMs: args["wait-for-all-gateways-time-ms"],
          maxTimestampDeviationMS: args["max-timestamp-deviation-ms"],
          authorizedSigners: getSignersForDataServiceId(
            args["authorized-signers"] ?? args["data-service-id"],
          ),
        });

        return {
          id: message.id,
          status: "success",
          data: payloadString,
        };
    }
  } catch (e) {
    console.error("Unknown error", e);
    return {
      id: message.id,
      status: "failure",
      message: e + "",
    };
  }
}
