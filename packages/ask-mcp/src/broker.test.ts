import { randomUUID } from "node:crypto";
import path from "node:path";

import { expect, onTestFinished, test, vi } from "vitest";

import {
  BrokerUnavailable,
  type BrokerOptions,
  Rejected,
  closeBroker,
  connect,
  createBroker,
  register,
} from "./broker.ts";
import { type FakeBrokerOptions, type Reply, fakeBroker, question, tempDir } from "./testing.ts";

/** fakeBroker と、それにつなぐ broker を用意する。broker はテストの終わりに閉じる */
const setup = async ({
  reply,
  eventsStatus,
  ...options
}: BrokerOptions & Pick<FakeBrokerOptions, "reply" | "eventsStatus"> = {}) => {
  const fake = await fakeBroker({ reply, eventsStatus });
  const client = randomUUID();
  const broker = createBroker(fake.socket, client, options);
  onTestFinished(() => closeBroker(broker));
  return { fake, client, broker };
};

test("shares one connection between calls and closes it", async () => {
  const { fake, client, broker } = await setup();

  await Promise.all([connect(broker), connect(broker)]);
  await connect(broker);
  expect(fake.streams.get(client)).toBe(1);
  const first = question({ clientId: client });
  await register(broker, first);
  expect(fake.received).toEqual([first]);

  closeBroker(broker);
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
});

test("connects again after the broker drops the connection", async () => {
  const { fake, client, broker } = await setup();
  await connect(broker);

  fake.server.closeAllConnections();
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  await expect(register(broker, question({ clientId: client }))).rejects.toBeInstanceOf(BrokerUnavailable);
  await connect(broker);
  await register(broker, question({ clientId: client }));
  expect(fake.received).toHaveLength(1);
});

test.each<[Reply, (new (...args: never[]) => Error) | undefined, string]>([
  [[200, ""], undefined, ""],
  [[400, "too many options"], Rejected, "too many options"],
  [[500, "boom"], Error, "hailr answered 500: boom"],
  ["cut", BrokerUnavailable, "cut off"],
])("reads the reply %j", async (reply, kind, message) => {
  const { fake, client, broker } = await setup({ reply: () => reply });
  await connect(broker);
  const registered = register(broker, question({ clientId: client }));
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
  let lost = 0;
  const { fake, client, broker } = await setup({ reply: () => [409, "no client"], onLost: () => lost++ });
  await connect(broker);
  await expect(register(broker, question({ clientId: client }))).rejects.toThrow("forgot this client");
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  expect(lost).toBe(1);
  await connect(broker);
  expect(fake.streams.get(client)).toBe(1);
});

test("closes a connection that is still opening", async () => {
  // ヘッダーが届く前に切れることを見るため、テストの期限より長く待たせる
  const fake = await fakeBroker({ headerDelayMs: 60_000 });
  const client = randomUUID();
  const broker = createBroker(fake.socket, client, { timeoutMs: 60_000 });
  const connecting = connect(broker);
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(1));
  closeBroker(broker);
  await expect(connecting).rejects.toBeInstanceOf(BrokerUnavailable);
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  await expect(connect(broker)).rejects.toThrow("closed");
});

test("gives up on a broker that is missing or silent", async () => {
  const missing = createBroker(path.join(tempDir(), "missing.sock"), randomUUID());
  await expect(connect(missing)).rejects.toBeInstanceOf(BrokerUnavailable);

  const { client, broker } = await setup({ reply: () => "silent", timeoutMs: 100 });
  await connect(broker);
  await expect(register(broker, question({ clientId: client }))).rejects.toBeInstanceOf(BrokerUnavailable);
});

test("reports a lost connection, but not a closed one", async () => {
  let lost = 0;
  const { fake, client, broker } = await setup({ onLost: () => lost++ });
  await connect(broker);
  fake.server.closeAllConnections();
  await vi.waitFor(() => expect(lost).toBe(1));
  await connect(broker);
  closeBroker(broker);
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  expect(lost).toBe(1);
});

test("hands events to the listener", async () => {
  const events: unknown[] = [];
  const { fake, client, broker } = await setup({ onEvent: (event) => events.push(event) });
  await connect(broker);

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
  const { fake, client, broker } = await setup();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  await connect(broker);
  fake.sse.get(client)?.write(`data: ${"x".repeat((1 << 20) + 1)}`);
  await vi.waitFor(() => expect(fake.streams.get(client)).toBe(0));
  expect(log).toHaveBeenCalledWith(expect.stringContaining("too long"));
});

test("gives up on events the broker does not serve", async () => {
  const { broker } = await setup({ eventsStatus: 503 });
  await expect(connect(broker)).rejects.toThrow("events: 503");
});

test("keeps a connection opened again while a question was on its way", async () => {
  let answer = (_: Reply) => {};
  let lost = 0;
  const { fake, client, broker } = await setup({
    reply: () => new Promise<Reply>((resolve) => (answer = resolve)),
    onLost: () => lost++,
  });
  await connect(broker);
  const registered = register(broker, question({ clientId: client }));
  await vi.waitFor(() => expect(fake.received).toHaveLength(1));
  fake.sse.get(client)?.destroy();
  await vi.waitFor(() => expect(lost).toBe(1));
  await connect(broker);

  answer([409, "no client"]);
  await expect(registered).rejects.toThrow("forgot this client");
  expect(lost).toBe(1);
  expect(broker.events).toBeDefined();
});
