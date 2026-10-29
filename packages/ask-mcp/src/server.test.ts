import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { expect, onTestFinished, test, vi } from "vitest";

import type { AskDeps } from "./ask.ts";
import { Rejected } from "./broker.ts";
import { createServer } from "./server.ts";
import { askDeps } from "./testing.ts";

const connectClient = async (deps: AskDeps) => {
  const server = createServer(() => deps);
  const client = new Client({ name: "test", version: "0" });
  const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverSide), client.connect(clientSide)]);
  onTestFinished(async () => {
    await client.close();
    await server.close();
  });
  return client;
};

const askHumanCall = { name: "ask_human", arguments: { title: "t", question: "q" } };

test("declares the channels Claude Code listens to", async () => {
  const client = await connectClient(askDeps().value);
  expect(client.getServerCapabilities()?.experimental).toEqual({
    "claude/channel": {},
    "claude/channel/permission": {},
  });
});

test("returns the question ID from ask_human", async () => {
  const deps = askDeps();
  const client = await connectClient(deps.value);
  const result = await client.callTool(askHumanCall);
  expect(result.isError).toBeFalsy();
  expect(JSON.stringify(result.content)).toContain("question ID q1");
  expect(deps.asked).toHaveLength(1);
});

test("returns why ask_human could not ask as an error", async () => {
  const refused = askDeps({
    ask: async () => {
      throw new Rejected("too many links");
    },
  });
  const client = await connectClient(refused.value);
  expect(await client.callTool(askHumanCall)).toMatchObject({
    isError: true,
    content: [{ type: "text", text: "hailr refused the question: too many links" }],
  });
});

test("refuses input that breaks the rules before asking", async () => {
  const deps = askDeps();
  const client = await connectClient(deps.value);
  const result = await client.callTool({ name: "ask_human", arguments: { title: " ", question: "q" } });
  expect(result.isError).toBe(true);
  expect(JSON.stringify(result.content)).toContain("must not be empty");
  expect(deps.asked).toEqual([]);
});

test("asks hailr about a permission request and logs a broken one", async () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const deps = askDeps();
  const client = await connectClient(deps.value);
  const method = "notifications/claude/channel/permission_request";
  const params = { request_id: "abcde", tool_name: "Bash", description: "d", input_preview: "{}" };
  await client.notification({ method, params: { ...params, request_id: "bad" } });
  await client.notification({ method, params });
  await vi.waitFor(() =>
    expect(deps.asked.map((q) => q.body)).toEqual([
      { kind: "permission", requestId: "abcde", toolName: "Bash", description: "d", inputPreview: "{}" },
    ]),
  );
  expect(log).toHaveBeenCalledTimes(1);
  expect(log).toHaveBeenCalledWith(expect.stringContaining("hailr:"));
});
