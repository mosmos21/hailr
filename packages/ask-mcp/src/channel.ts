import * as z from "zod";

/** Claude Code が channel として通知を受け付け、権限確認も送ってくるために要る宣言 */
export const channelCapabilities = {
  experimental: { "claude/channel": {}, "claude/channel/permission": {} },
};

export type ChannelNotification = { method: string; params: Record<string, unknown> };

export const channelNotification = (content: string, meta: Record<string, string>) => {
  return { method: "notifications/claude/channel", params: { content, meta } };
};

export const permissionVerdict = (requestId: string, behavior: "allow" | "deny") => {
  return {
    method: "notifications/claude/channel/permission",
    params: { request_id: requestId, behavior },
  };
};

export const permissionRequest = z.object({
  method: z.literal("notifications/claude/channel/permission_request"),
  params: z.object({
    request_id: z.string().regex(/^[a-km-z]{5}$/),
    tool_name: z.string().min(1),
    description: z.string(),
    input_preview: z.string(),
  }),
});

export type PermissionRequest = z.infer<typeof permissionRequest>["params"];
