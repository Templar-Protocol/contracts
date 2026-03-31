import z from "zod";

export const Request = z.discriminatedUnion("method", [
  z.object({
    id: z.uint32(),
    method: z.literal("fetch"),
    params: z.array(z.string()),
  }),
]);
export type Request = z.infer<typeof Request>;

export const Response = z.discriminatedUnion("status", [
  z.object({
    id: z.uint32(),
    status: z.literal("success"),
    data: z.string(),
  }),
  z.object({
    id: z.uint32(),
    status: z.literal("failure"),
    message: z.string(),
  }),
]);
export type Response = z.infer<typeof Response>;
