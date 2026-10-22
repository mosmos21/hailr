import { randomUUID } from "node:crypto";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

import { askHuman, askInput, description } from "./ask.ts";
import { Broker } from "./broker.ts";
import { channelCapabilities, permissionRequest } from "./channel.ts";
import { socketPath } from "./paths.ts";
import { askPermission } from "./permission.ts";
import { Session } from "./session.ts";

const server = new McpServer({ name: "hailr", version: "0.1.0" }, { capabilities: channelCapabilities });
const socket = socketPath(process.env);
const session = socket
  ? new Session(new Broker(socket, randomUUID()), (n) => server.server.notification(n))
  : undefined;
const deps = () => ({ session, paneId: process.env.HERDR_PANE_ID, now: Date.now, newId: randomUUID });

// 形の崩れた通知は SDK が onerror に回すため、黙って消えないようにログに出す
server.server.onerror = (e) => console.error(`hailr: ${String(e)}`);
server.server.setNotificationHandler(permissionRequest, ({ params }) => askPermission(params, deps()));

server.registerTool("ask_human", { description, inputSchema: askInput }, async (input) => {
  try {
    const text = await askHuman(input, deps());
    return { content: [{ type: "text", text }] };
  } catch (e) {
    const text = e instanceof Error ? e.message : String(e);
    return { isError: true, content: [{ type: "text", text }] };
  }
});

await server.connect(new StdioServerTransport());
// SSE の接続が残ると、Claude Code が stdin を閉じてもプロセスが終わらないため切る
process.stdin.once("end", () => {
  session?.close();
  void server.close();
});
