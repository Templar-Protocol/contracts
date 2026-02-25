import * as net from "node:net";
import * as readline from "node:readline";
import { parseArgs } from "./args.js";
import { Request } from "./msg.js";
import handle from "./handle.js";

const args = parseArgs(process.argv.slice(2));

console.log("Connecting to relayer", args.socket);

const client = net.createConnection({ path: args.socket });

client.on("error", (e) => {
  console.error("Connection error", e);
  process.exit(1);
});

const rl = readline.createInterface({
  input: client,
  terminal: false,
});

rl.on("line", async (data) => {
  try {
    const message = Request.parse(JSON.parse(data));
    const response = await handle(args, message);
    client.write(JSON.stringify(response) + "\n");
  } catch (e) {
    console.error("Failed to process message", data, e);
    process.exit(1);
  }
});

client.on("end", () => {
  console.log("Disconnected from relayer");
  process.exit(0);
});
