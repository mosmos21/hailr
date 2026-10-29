import { randomUUID } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";

import { onTestFinished } from "vitest";

import type { AskDeps } from "./ask.ts";
import type { Question } from "./broker.ts";

export const tempDir = () => {
  const dir = mkdtempSync(path.join(tmpdir(), "hailr-"));
  onTestFinished(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
};

export type Reply = [number, string] | "silent" | "cut";

/** broker の代わりに、SSE をつないだ client の質問だけを受け付ける */
export type FakeBrokerOptions = {
  reply?: () => Reply | Promise<Reply>;
  headerDelayMs?: number;
  eventsStatus?: number;
};

export const fakeBroker = async ({
  reply = () => [201, ""],
  headerDelayMs = 0,
  eventsStatus = 200,
}: FakeBrokerOptions = {}) => {
  const sse = new Map<string, http.ServerResponse>();
  const socket = path.join(tempDir(), "broker.sock");
  const streams = new Map<string, number>();
  const received: Question[] = [];
  const server = http.createServer((request, response) => {
    const events = request.url?.match(/^\/clients\/([^/]+)\/events$/);
    if (request.method === "GET" && events?.[1]) {
      const client = events[1];
      if (eventsStatus !== 200) {
        response.writeHead(eventsStatus).end();
        return;
      }
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
    const answer = async () => {
      const question = JSON.parse(body) as Question;
      if (!streams.get(question.clientId)) {
        response.writeHead(409).end("no client");
        return;
      }
      received.push(question);
      const replied = await reply();
      if (replied === "cut") {
        response.writeHead(201, { "content-length": "100" });
        response.write("partial", () => response.socket?.destroy());
      } else if (replied !== "silent") {
        response.writeHead(replied[0]).end(replied[1]);
      }
    };
    request.on("end", () => {
      answer().catch((e: unknown) => response.writeHead(500).end(String(e)));
    });
  });
  await new Promise<void>((resolve) => server.listen(socket, resolve));
  onTestFinished(() => {
    server.closeAllConnections();
    server.close();
  });
  return { socket, received, server, streams, sse };
};

export const fixture = (name: string): unknown => {
  const url = new URL(`../../../fixtures/protocol/${name}`, import.meta.url);
  return JSON.parse(readFileSync(url, "utf8"));
};

export const question = ({
  id = randomUUID(),
  clientId = "c1",
  title = "t",
}: { id?: string; clientId?: string; title?: string } = {}): Question => ({
  id,
  clientId,
  paneId: "w1:p1",
  askedAt: 1,
  body: { kind: "ask", title, question: "q", options: [], links: [] },
});

export const permission = ({ id, requestId }: { id: string; requestId: string }): Question => ({
  ...question({ id }),
  body: { kind: "permission", requestId, toolName: "Bash", description: "d", inputPreview: "p" },
});

/** expected と同じ id・client・pane・時刻で質問を組み立てさせ、ask に渡された質問を asked に残す */
export const askDeps = ({
  expected = question({ id: "q1" }),
  ask = async () => {},
}: {
  expected?: Pick<Question, "id" | "clientId" | "paneId" | "askedAt">;
  ask?: (q: Question) => Promise<void>;
} = {}) => {
  const asked: Question[] = [];
  const value: AskDeps = {
    asker: {
      clientId: expected.clientId,
      ask: async (q) => {
        asked.push(q);
        await ask(q);
      },
    },
    paneId: expected.paneId,
    now: () => expected.askedAt,
    newId: () => expected.id,
  };
  return { value, asked };
};
