import net from "node:net";
import { parseArgs } from "./args.js";
import { Request } from "./msg.js";
import handle from "./handle.js";

const args = parseArgs(process.argv.slice(2));

console.log("Connecting to relayer", args.socket);

const client = net.createConnection({ path: args.socket });

client.on("data", async (data) => {
  const dataStr = data.toString();
  const message = Request.parse(JSON.parse(dataStr));

  const response = await handle(args, message);

  client.write(JSON.stringify(response) + "\n");
});

client.on("end", () => {
  console.log("Disconnected from relayer");
  process.exit(0);
});
