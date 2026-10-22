import { expect, test } from "vitest";

import { socketPath } from "./paths.ts";

test("uses an absolute XDG_STATE_HOME, then HOME", () => {
  expect(socketPath({ XDG_STATE_HOME: "/s", HOME: "/h" })).toBe("/s/hailr/broker.sock");
  expect(socketPath({ XDG_STATE_HOME: "rel", HOME: "/h" })).toBe(
    "/h/.local/state/hailr/broker.sock",
  );
  expect(socketPath({ HOME: "/h" })).toBe("/h/.local/state/hailr/broker.sock");
  expect(socketPath({ HOME: "rel" })).toBeUndefined();
  expect(socketPath({})).toBeUndefined();
});
