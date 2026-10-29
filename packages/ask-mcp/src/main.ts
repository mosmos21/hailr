import { randomUUID } from "node:crypto";

import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

import type { Question } from "./broker.ts";
import { socketPath } from "./paths.ts";
import { createServer } from "./server.ts";
import { ask, closeSession } from "./session.ts";
import { startSession } from "./start.ts";

const server = createServer(() => ({
  asker,
  paneId: process.env.HERDR_PANE_ID,
  now: Date.now,
  newId: randomUUID,
}));
const socket = socketPath(process.env);
const started = socket ? startSession(socket, (n) => server.server.notification(n)) : undefined;
const asker = started && { clientId: started.clientId, ask: (q: Question) => ask(started.session, q) };

await server.connect(new StdioServerTransport());
// SSE の接続が残ると、Claude Code が stdin を閉じてもプロセスが終わらないため切る
process.stdin.once("end", () => {
  if (started) closeSession(started.session);
  void server.close();
});
