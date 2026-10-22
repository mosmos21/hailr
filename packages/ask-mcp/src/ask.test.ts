import { readFileSync } from "node:fs";

import { expect, test } from "vitest";
import * as z from "zod";

import { type AskDeps, askHuman, askInput } from "./ask.ts";
import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";

function deps(register: (q: Question) => Promise<void> = async () => {}) {
  const registered: Question[] = [];
  const calls: string[] = [];
  const value: AskDeps = {
    broker: {
      clientId: "c1",
      connect: async () => {
        calls.push("connect");
      },
      register: async (q) => {
        calls.push("register");
        registered.push(q);
        await register(q);
      },
    },
    paneId: "w1:p1",
    now: () => 42,
    newId: () => "q1",
  };
  return { value, registered, calls };
}

function fixture(name: string): Question {
  const url = new URL(`../../../fixtures/protocol/${name}`, import.meta.url);
  return JSON.parse(readFileSync(url, "utf8")) as Question;
}

test.each(["question-ask.json", "question-ask-minimal.json"])(
  "sends what the protocol fixture %s shows",
  async (name) => {
    const expected = fixture(name);
    const { value, registered, calls } = deps();
    const { title, question, options, links } = expected.body;
    const input = {
      title,
      question,
      ...(options.length > 0 && { options }),
      ...(links.length > 0 && { links }),
    };
    await askHuman(input, {
      ...value,
      broker: value.broker && { ...value.broker, clientId: expected.clientId },
      paneId: expected.paneId,
      now: () => expected.askedAt,
      newId: () => expected.id,
    });
    expect(registered).toEqual([expected]);
    expect(calls).toEqual(["connect", "register"]);
  },
);

test("registers the question for this pane and returns its ID", async () => {
  const { value, registered } = deps();
  const text = await askHuman({ title: "Merge?", question: "Merge #1?", options: ["Yes"] }, value);
  expect(text).toContain("q1");
  expect(registered).toEqual([
    {
      id: "q1",
      clientId: "c1",
      paneId: "w1:p1",
      askedAt: 42,
      body: { kind: "ask", title: "Merge?", question: "Merge #1?", options: ["Yes"], links: [] },
    },
  ]);
});

test("explains why it cannot ask", async () => {
  const input = { title: "t", question: "q" };
  for (const paneId of [undefined, "", "-x"]) {
    await expect(askHuman(input, { ...deps().value, paneId })).rejects.toThrow("herdr pane");
  }
  await expect(askHuman(input, { ...deps().value, broker: undefined })).rejects.toThrow("socket");
  const down = deps(async () => {
    throw new BrokerUnavailable("ENOENT");
  });
  await expect(askHuman(input, down.value)).rejects.toThrow("hailr is not running");
  const unreachable = deps();
  const broker = unreachable.value.broker;
  const failing = broker && {
    ...broker,
    connect: async () => {
      throw new BrokerUnavailable("ENOENT");
    },
  };
  await expect(askHuman(input, { ...unreachable.value, broker: failing })).rejects.toThrow(
    "hailr is not running",
  );
  expect(unreachable.registered).toEqual([]);
  const refused = deps(async () => {
    throw new Rejected("too many links");
  });
  await expect(askHuman(input, refused.value)).rejects.toThrow("refused the question: too many links");
});

test("checks the input the way the broker does", () => {
  const schema = z.object(askInput);
  const ok = (value: unknown) => schema.safeParse(value).success;
  expect(ok({ title: "t", question: "q", options: ["a", "b"], links: ["https://x.example/"] })).toBe(
    true,
  );
  expect(ok({ title: " ", question: "q" })).toBe(false);
  expect(ok({ title: "t", question: "q", options: ["a", "a"] })).toBe(false);
  expect(ok({ title: "t", question: "q", options: [""] })).toBe(false);
  expect(ok({ title: "t", question: "q", options: Array.from({ length: 10 }, (_, i) => `${i}`) })).toBe(
    false,
  );
  expect(ok({ title: "t", question: "q", links: ["file:///etc/passwd"] })).toBe(false);
  expect(ok({ title: "t", question: "q", links: Array(6).fill("https://x.example/") })).toBe(false);
});
