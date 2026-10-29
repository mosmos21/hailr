import * as z from "zod";

/** broker が SSE で送る出来事。fixtures/protocol/event-*.json と同じ形 */
export const clientEvent = z.discriminatedUnion("type", [
  z.object({ type: z.literal("answered"), id: z.string(), answer: z.string() }),
  z.object({
    type: z.literal("withdrawn"),
    id: z.string(),
    reason: z.enum(["deleted", "answered_in_pane", "pane_closed"]),
  }),
]);

export type ClientEvent = z.infer<typeof clientEvent>;

/** 区切りの空行までを 1 件として data 行を取り出す。読み切れていない残りも返す */
export const readSse = (buffer: string): { data: string[]; rest: string } => {
  const blocks = buffer.split(/\r?\n\r?\n/);
  const rest = blocks.pop() ?? "";
  const data = blocks.flatMap((block) => {
    const lines = block
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).replace(/^ /, ""));
    return lines.length > 0 ? [lines.join("\n")] : [];
  });
  return { data, rest };
};
