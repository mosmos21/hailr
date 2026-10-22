import { readFileSync } from "node:fs";

import { expect, test } from "vitest";
import * as z from "zod";

import { type AskDeps, askHuman, askInput } from "./ask.ts";
import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";

function deps(ask: (q: Question) => Promise<void> = async () => {}) {
  const asked: Question[] = [];
  const value: AskDeps = {
    session: {
      clientId: "c1",
      ask: async (q) => {
        asked.push(q);
        await ask(q);
      },
    },
    paneId: "w1:p1",
    now: () => 42,
    newId: () => "q1",
  };
  return { value, asked };
}

function fixture(name: string): Question {
  const url = new URL(`../../../fixtures/protocol/${name}`, import.meta.url);
  return JSON.parse(readFileSync(url, "utf8")) as Question;
}

test.each(["question-ask.json", "question-ask-minimal.json"])(
  "sends what the protocol fixture %s shows",
  async (name) => {
    const expected = fixture(name);
    const { value, asked } = deps();
    const { title, question, options, links } = expected.body;
    const input = {
      title,
      question,
      ...(options.length > 0 && { options }),
      ...(links.length > 0 && { links }),
    };
    const text = await askHuman(input, {
      ...value,
      session: value.session && { ...value.session, clientId: expected.clientId },
      paneId: expected.paneId,
      now: () => expected.askedAt,
      newId: () => expected.id,
    });
    expect(asked).toEqual([expected]);
    expect(text).toContain(expected.id);
  },
);

test("explains why it cannot ask", async () => {
  const input = { title: "t", question: "q" };
  for (const paneId of [undefined, "", "-x"]) {
    await expect(askHuman(input, { ...deps().value, paneId })).rejects.toThrow("herdr pane");
  }
  await expect(askHuman(input, { ...deps().value, session: undefined })).rejects.toThrow("socket");
  const down = deps(async () => {
    throw new BrokerUnavailable("ENOENT");
  });
  await expect(askHuman(input, down.value)).rejects.toThrow("hailr is not running");
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
