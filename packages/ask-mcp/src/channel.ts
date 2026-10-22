/** Claude Code が channel として通知を受け付けるために要る宣言 */
export const channelCapabilities = { experimental: { "claude/channel": {} } };

export function channelNotification(content: string, meta: Record<string, string>) {
  return { method: "notifications/claude/channel", params: { content, meta } };
}
