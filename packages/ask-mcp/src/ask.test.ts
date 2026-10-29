import { expect, test } from "vitest";
import * as z from "zod";

import { askHuman, askInput } from "./ask.ts";
import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";
import { askDeps, fixture } from "./testing.ts";

test.each(["question-ask.json", "question-ask-minimal.json"])(
  "sends what the protocol fixture %s shows",
  async (name) => {
    const expected = fixture(name) as Question;
    const { value, asked } = askDeps({ expected });
    if (expected.body.kind !== "ask") throw new Error("fixture is not an ask");
    const { title, question, options, links } = expected.body;
    const input = {
      title,
      question,
      ...(options.length > 0 && { options }),
      ...(links.length > 0 && { links }),
    };
    const text = await askHuman(input, value);
    expect(asked).toEqual([expected]);
    expect(text).toContain(expected.id);
  },
);

test("explains why it cannot ask", async () => {
  const input = { title: "t", question: "q" };
  for (const paneId of [undefined, "", "-x"]) {
    await expect(askHuman(input, { ...askDeps().value, paneId })).rejects.toThrow("herdr pane");
  }
  await expect(askHuman(input, { ...askDeps().value, asker: undefined })).rejects.toThrow("socket");
  const down = askDeps({
    ask: async () => {
      throw new BrokerUnavailable("ENOENT");
    },
  });
  await expect(askHuman(input, down.value)).rejects.toThrow("hailr is not running");
  const refused = askDeps({
    ask: async () => {
      throw new Rejected("too many links");
    },
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
