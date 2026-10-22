import { randomUUID } from "node:crypto";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

import { askHuman, askInput, description } from "./ask.ts";
import { Broker } from "./broker.ts";
import { socketPath } from "./paths.ts";

const socket = socketPath(process.env);
const broker = socket ? new Broker(socket, randomUUID()) : undefined;
const server = new McpServer({ name: "hailr", version: "0.1.0" });

server.registerTool("ask_human", { description, inputSchema: askInput }, async (input) => {
  try {
    const text = await askHuman(input, {
      broker,
      paneId: process.env.HERDR_PANE_ID,
      now: Date.now,
      newId: randomUUID,
    });
    return { content: [{ type: "text", text }] };
  } catch (e) {
    const text = e instanceof Error ? e.message : String(e);
    return { isError: true, content: [{ type: "text", text }] };
  }
});

await server.connect(new StdioServerTransport());
// SSE の接続が残ると、Claude Code が stdin を閉じてもプロセスが終わらないため切る
process.stdin.once("end", () => {
  broker?.close();
  void server.close();
});
