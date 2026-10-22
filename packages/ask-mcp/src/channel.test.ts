import { expect, test } from "vitest";

import {
  channelCapabilities,
  channelNotification,
  permissionRequest,
  permissionVerdict,
} from "./channel.ts";

test("speaks the channel protocol of Claude Code", () => {
  expect(channelCapabilities).toEqual({
    experimental: { "claude/channel": {}, "claude/channel/permission": {} },
  });
  expect(channelNotification("hi", { question_id: "q1" })).toEqual({
    method: "notifications/claude/channel",
    params: { content: "hi", meta: { question_id: "q1" } },
  });
  expect(permissionVerdict("abcde", "deny")).toEqual({
    method: "notifications/claude/channel/permission",
    params: { request_id: "abcde", behavior: "deny" },
  });
});

test("reads permission requests and refuses broken ones", () => {
  const params = { request_id: "abcde", tool_name: "Bash", description: "d", input_preview: "{}" };
  const method = "notifications/claude/channel/permission_request";
  expect(permissionRequest.parse({ method, params }).params).toEqual(params);
  for (const broken of [
    { request_id: "" },
    { request_id: "abcdl" },
    { request_id: "ABCDE" },
    { request_id: "abcd" },
    { request_id: "abcdef" },
    { request_id: " abcde" },
    { request_id: 1 },
    { tool_name: "" },
  ]) {
    expect(permissionRequest.safeParse({ method, params: { ...params, ...broken } }).success).toBe(false);
  }
  expect(permissionRequest.safeParse({ method, params: { ...params, description: "" } }).success).toBe(true);
  expect(permissionRequest.safeParse({ method, params: { request_id: "abcde" } }).success).toBe(false);
});
