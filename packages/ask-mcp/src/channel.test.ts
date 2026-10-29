import { expect, test } from "vitest";

import { permissionRequest } from "./channel.ts";

test("reads permission requests and refuses broken ones", () => {
  const params = { request_id: "abcde", tool_name: "Bash", description: "", input_preview: "{}" };
  const method = "notifications/claude/channel/permission_request";
  expect(permissionRequest.parse({ method, params }).params).toEqual(params);
  for (const broken of [
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
  expect(permissionRequest.safeParse({ method, params: { request_id: "abcde" } }).success).toBe(false);
});
