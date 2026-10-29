import { expect, test } from "vitest";

import { clientEvent, readSse } from "./events.ts";
import { fixture } from "./testing.ts";

test.each([
  "event-answered.json",
  "event-withdrawn.json",
  "event-withdrawn-deleted.json",
  "event-withdrawn-pane-closed.json",
])("reads the protocol fixture %s", (name) => {
  expect(clientEvent.safeParse(fixture(name)).success).toBe(true);
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
