import { readFileSync } from "node:fs";

import { expect, test } from "vitest";

import { clientEvent, readSse } from "./events.ts";

test.each([
  "event-answered.json",
  "event-withdrawn.json",
  "event-withdrawn-deleted.json",
  "event-withdrawn-pane-closed.json",
])("reads the protocol fixture %s", (name) => {
  const url = new URL(`../../../fixtures/protocol/${name}`, import.meta.url);
  expect(clientEvent.safeParse(JSON.parse(readFileSync(url, "utf8"))).success).toBe(true);
});

test("splits SSE into data and keeps the unfinished part", () => {
  expect(readSse(':\n\ndata: {"a":1}\n\ndata:{"b"')).toEqual({
    data: ['{"a":1}'],
    rest: 'data:{"b"',
  });
  expect(readSse("data: x\r\ndata: y\r\n\r\n")).toEqual({ data: ["x\ny"], rest: "" });
});

test.each([
  { type: "withdrawn", id: "q1", reason: "expired" },
  { type: "answered", id: "q1" },
  { type: "answered", answer: "Yes" },
])("refuses the event %j", (event) => {
  expect(clientEvent.safeParse(event).success).toBe(false);
});
