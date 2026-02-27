import type { Args } from "./args.js";
import type { Request, Response } from "./msg.js";
import {
  requestRedstonePayload,
  getSignersForDataServiceId,
} from "@redstone-finance/sdk";

export default async function handle(
  args: Args,
  request: Request,
): Promise<Response> {
  try {
    switch (request.method) {
      case "fetch":
        console.debug("Fetching", request.params);

        const payloadString = await requestRedstonePayload({
          dataServiceId: args["data-service-id"],
          dataPackagesIds: request.params,
          uniqueSignersCount: args["unique-signers-count"],
          waitForAllGatewaysTimeMs: args["wait-for-all-gateways-time-ms"],
          maxTimestampDeviationMS: args["max-timestamp-deviation-ms"],
          authorizedSigners: getSignersForDataServiceId(
            args["authorized-signers"] ?? args["data-service-id"],
          ),
        });

        return {
          id: request.id,
          status: "success",
          data: payloadString,
        };
    }
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    console.error("Unknown error", e);
    return {
      id: request.id,
      status: "failure",
      message,
    };
  }
}
