import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";

import { type AskDeps, askHuman, askInput, description } from "./ask.ts";
import { channelCapabilities, permissionRequest } from "./channel.ts";
import { askPermission } from "./permission.ts";

/** ask_human と権限確認の通知を受け付ける MCP サーバー。deps は呼ばれるたびに取り直す */
export const createServer = (deps: () => AskDeps): McpServer => {
  const server = new McpServer({ name: "hailr", version: "0.1.0" }, { capabilities: channelCapabilities });

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

  return server;
};
