import * as z from "zod";

import { type Broker, BrokerUnavailable, Rejected } from "./broker.ts";

const text = z.string().refine((s) => s.trim() !== "", "must not be empty");

/** broker と同じ規則を先に当て、Claude に分かる言葉で返す */
export const askInput = {
  title: text.describe("A few words for the question list"),
  question: text.describe("The full question"),
  options: z
    .array(text)
    .max(9)
    .refine((o) => new Set(o).size === o.length, "options must be distinct")
    .optional()
    .describe("Fixed answers shown as buttons; the chosen text comes back as the answer"),
  links: z
    .array(z.url({ protocol: /^https?$/ }))
    .max(5)
    .optional()
    .describe("Related pages such as pull requests or issues"),
};

export type AskInput = z.infer<z.ZodObject<typeof askInput>>;

export type AskDeps = {
  broker: Pick<Broker, "connect" | "register" | "clientId"> | undefined;
  paneId: string | undefined;
  now: () => number;
  newId: () => string;
};

export const description = [
  "Ask the human a question through hailr instead of waiting in this pane.",
  "Returns at once with a question ID.",
  "End your turn without doing work that depends on the answer; the answer arrives later as a channel message.",
].join(" ");

/** 登録できたら Claude に返す文、できなければ理由を投げる */
export async function askHuman(input: AskInput, deps: AskDeps): Promise<string> {
  const { broker, paneId } = deps;
  if (!paneId || paneId.startsWith("-")) {
    throw new Error("ask_human works only inside a herdr pane; ask in the pane instead");
  }
  if (!broker) {
    throw new Error("cannot find the hailr socket (HOME is not set); ask in the pane instead");
  }
  const id = deps.newId();
  try {
    await broker.connect();
    await broker.register({
      id,
      clientId: broker.clientId,
      paneId,
      askedAt: deps.now(),
      body: {
        kind: "ask",
        title: input.title,
        question: input.question,
        options: input.options ?? [],
        links: input.links ?? [],
      },
    });
  } catch (e) {
    if (e instanceof Rejected) {
      throw new Error(`hailr refused the question: ${e.message}`);
    }
    if (e instanceof BrokerUnavailable) {
      console.error(`hailr: ${e.message}`);
      throw new Error("hailr is not running; ask in the pane instead");
    }
    throw e;
  }
  return `Asked (question ID ${id}). End your turn now; the answer will arrive as a channel message.`;
}
