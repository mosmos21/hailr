import { expect, test } from "vitest";

import { channelCapabilities, channelNotification } from "./channel.ts";

test("speaks the channel protocol of Claude Code", () => {
  expect(channelCapabilities).toEqual({ experimental: { "claude/channel": {} } });
  expect(channelNotification("hi", { question_id: "q1" })).toEqual({
    method: "notifications/claude/channel",
    params: { content: "hi", meta: { question_id: "q1" } },
  });
});
