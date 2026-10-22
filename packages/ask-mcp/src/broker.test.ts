import { randomUUID } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, expect, onTestFinished, test, vi } from "vitest";

import { Broker, BrokerUnavailable, type Question, Rejected } from "./broker.ts";

const cleanups: (() => void)[] = [];
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) {
    cleanup();
  }
});

function tempDir() {
  const dir = mkdtempSync(path.join(tmpdir(), "hailr-"));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

type Reply = [number, string] | "silent" | "cut";

/** broker の代わりに、SSE をつないだ client の質問だけを受け付ける */
async function fakeBroker(reply: () => Reply = () => [201, ""], headerDelayMs = 0) {
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
  cleanups.push(() => {
    server.closeAllConnections();
    server.close();
  });
  return { socket, received, server, streams, sse };
}

function question(clientId: string): Question {
  return {
    id: randomUUID(),
    clientId,
    paneId: "w1:p1",
    askedAt: 1,
    body: { kind: "ask", title: "t", question: "q", options: ["Yes"], links: [] },
  };
}

test("shares one connection between calls and closes it", async () => {
  const fake = await fakeBroker();
  const client = randomUUID();
  const broker = new Broker(fake.socket, client);
  cleanups.push(() => broker.close());

  await Promise.all([broker.connect(), broker.connect()]);
  await broker.connect();
  expect(fake.streams.get(client)).toBe(1);
  const first = question(client);
  await broker.register(first);
  expect(fake.received).toEqual([first]);

  broker.close();
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
});

test("connects again after the broker drops the connection", async () => {
  const fake = await fakeBroker();
  const client = randomUUID();
  const broker = new Broker(fake.socket, client);
  cleanups.push(() => broker.close());
  await broker.connect();

  fake.server.closeAllConnections();
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  await expect(broker.register(question(client))).rejects.toBeInstanceOf(BrokerUnavailable);
  await broker.connect();
  await broker.register(question(client));
  expect(fake.received).toHaveLength(1);
});

test.each<[Reply, (new (...args: never[]) => Error) | undefined, string]>([
  [[200, ""], undefined, ""],
  [[400, "too many options"], Rejected, "too many options"],
  [[500, "boom"], Error, "hailr answered 500: boom"],
  ["cut", BrokerUnavailable, "cut off"],
])("reads the reply %j", async (reply, kind, message) => {
  const fake = await fakeBroker(() => reply);
  const client = randomUUID();
  const broker = new Broker(fake.socket, client);
  cleanups.push(() => broker.close());
  await broker.connect();
  const registered = broker.register(question(client));
  if (!kind) {
    await expect(registered).resolves.toBeUndefined();
    return;
  }
  const error = await registered.catch((e: unknown) => e);
  expect(error).toBeInstanceOf(kind);
  expect(Object.getPrototypeOf(error)).toBe(kind.prototype);
  expect((error as Error).message).toContain(message);
});

test("drops a connection the broker no longer knows", async () => {
  const fake = await fakeBroker(() => [409, "no client"]);
  const client = randomUUID();
  const broker = new Broker(fake.socket, client);
  cleanups.push(() => broker.close());
  let lost = 0;
  broker.onLost = () => lost++;
  await broker.connect();
  await expect(broker.register(question(client))).rejects.toThrow("forgot this client");
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  expect(lost).toBe(1);
  await broker.connect();
  expect(fake.streams.get(client)).toBe(1);
});

test("closes a connection that is still opening", async () => {
  // ヘッダーが届く前に切れることを見るため、テストの期限より長く待たせる
  const fake = await fakeBroker(undefined, 60_000);
  const client = randomUUID();
  const broker = new Broker(fake.socket, client, 60_000);
  const connecting = broker.connect();
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(1));
  broker.close();
  await expect(connecting).rejects.toBeInstanceOf(BrokerUnavailable);
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  await expect(broker.connect()).rejects.toThrow("closed");
});

test("gives up on a broker that is missing or silent", async () => {
  const missing = new Broker(path.join(tempDir(), "missing.sock"), randomUUID());
  await expect(missing.connect()).rejects.toBeInstanceOf(BrokerUnavailable);

  const fake = await fakeBroker(() => "silent");
  const client = randomUUID();
  const broker = new Broker(fake.socket, client, 100);
  cleanups.push(() => broker.close());
  await broker.connect();
  await expect(broker.register(question(client))).rejects.toBeInstanceOf(BrokerUnavailable);
});

test("reports a lost connection, but not a closed one", async () => {
  const fake = await fakeBroker();
  const client = randomUUID();
  const broker = new Broker(fake.socket, client);
  cleanups.push(() => broker.close());
  let lost = 0;
  broker.onLost = () => lost++;
  await broker.connect();
  fake.server.closeAllConnections();
  await vi.waitFor(() => expect(lost).toBe(1));
  await broker.connect();
  broker.close();
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  expect(lost).toBe(1);
});

test("hands events to the listener", async () => {
  const fake = await fakeBroker();
  const client = randomUUID();
  const broker = new Broker(fake.socket, client);
  cleanups.push(() => broker.close());
  const events: unknown[] = [];
  broker.onEvent = (event) => events.push(event);
  await broker.connect();

  const stream = fake.sse.get(client);
  const yes = Buffer.from('data: {"type":"answered","id":"q1","answer":"はい"}\n\n');
  const cut = yes.indexOf(Buffer.from("は")) + 1;
  stream?.write(yes.subarray(0, cut));
  await new Promise((resolve) => setTimeout(resolve, 20));
  stream?.write(yes.subarray(cut));
  await vi.waitFor(() => expect(events).toHaveLength(1));
  stream?.write('data: not json\n\ndata: {"type":"other"}\n\n');
  await new Promise((resolve) => setTimeout(resolve, 20));
  stream?.write('data: {"type":"withdrawn","id":"q2","reason":"deleted"}\n\n');
  await vi.waitFor(() =>
    expect(events).toEqual([
      { type: "answered", id: "q1", answer: "はい" },
      { type: "withdrawn", id: "q2", reason: "deleted" },
    ]),
  );
});

test("drops a connection that sends an endless event", async () => {
  const fake = await fakeBroker();
  const client = randomUUID();
  const broker = new Broker(fake.socket, client);
  cleanups.push(() => broker.close());
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  await broker.connect();
  fake.sse.get(client)?.write(`data: ${"x".repeat((1 << 20) + 1)}`);
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  expect(log).toHaveBeenCalledWith(expect.stringContaining("too long"));
});
