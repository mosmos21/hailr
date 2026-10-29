import { randomUUID } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";

import { onTestFinished } from "vitest";

import type { Question } from "./broker.ts";

export const tempDir = () => {
  const dir = mkdtempSync(path.join(tmpdir(), "hailr-"));
  onTestFinished(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
};

export type Reply = [number, string] | "silent" | "cut";

/** broker の代わりに、SSE をつないだ client の質問だけを受け付ける */
export const fakeBroker = async (reply: () => Reply = () => [201, ""], headerDelayMs = 0) => {
  const sse = new Map<string, http.ServerResponse>();
  const socket = path.join(tempDir(), "broker.sock");
  const streams = new Map<string, number>();
  const received: Question[] = [];
  const server = http.createServer((request, response) => {
    const events = request.url?.match(/^\/clients\/([^/]+)\/events$/);
    if (request.method === "GET" && events?.[1]) {
      const client = events[1];
      streams.set(client, (streams.get(client) ?? 0) + 1);
      const headers = setTimeout(() => {
        response.writeHead(200, { "content-type": "text/event-stream" });
        response.write(":\n\n");
        sse.set(client, response);
      }, headerDelayMs);
      request.on("close", () => {
        clearTimeout(headers);
        streams.set(client, (streams.get(client) ?? 1) - 1);
      });
      return;
    }
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      const question = JSON.parse(body) as Question;
      if (!streams.get(question.clientId)) {
        response.writeHead(409).end("no client");
        return;
      }
      received.push(question);
      const answer = reply();
      if (answer === "cut") {
        response.writeHead(201, { "content-length": "100" });
        response.write("partial", () => response.socket?.destroy());
      } else if (answer !== "silent") {
        response.writeHead(answer[0]).end(answer[1]);
      }
    });
  });
  await new Promise<void>((resolve) => server.listen(socket, resolve));
  onTestFinished(() => {
    server.closeAllConnections();
    server.close();
  });
  return { socket, received, server, streams, sse };
};

export const question = (clientId: string): Question => {
  return {
    id: randomUUID(),
    clientId,
    paneId: "w1:p1",
    askedAt: 1,
    body: { kind: "ask", title: "t", question: "q", options: ["Yes"], links: [] },
  };
};
