import type { Broker, Question } from "./broker.ts";
import type { ClientEvent } from "./events.ts";

export type Notify = (content: string, meta: Record<string, string>) => Promise<void>;

type SessionBroker = Pick<Broker, "connect" | "register" | "close" | "clientId" | "onEvent">;

/** 未回答の質問を持ち、届いた回答を Claude に返す */
export class Session {
  readonly #broker: SessionBroker;
  readonly #notify: Notify;
  readonly #pending = new Map<string, Question>();

  constructor(broker: SessionBroker, notify: Notify) {
    this.#broker = broker;
    this.#notify = notify;
    broker.onEvent = (event) => this.#handle(event);
  }

  get clientId(): string {
    return this.#broker.clientId;
  }

  pending(): Question[] {
    return [...this.#pending.values()];
  }

  /** 登録の直後に届く回答を取りこぼさないよう、送る前に手元に置く。
   * 失敗は ask_human のエラーとして Claude に返り、Claude が聞き直すので、broker に届いていても手元からは消す */
  async ask(question: Question): Promise<void> {
    this.#pending.set(question.id, question);
    try {
      await this.#broker.connect();
      await this.#broker.register(question);
    } catch (e) {
      this.#pending.delete(question.id);
      throw e;
    }
  }

  #handle(event: ClientEvent): void {
    const question = this.#pending.get(event.id);
    if (!question) {
      console.error(`hailr: an event for a question this session does not hold: ${event.id}`);
      return;
    }
    this.#pending.delete(event.id);
    const title = question.body.title;
    const meta = { question_id: event.id };
    let sent: Promise<void> | undefined;
    if (event.type === "answered") {
      const content = `The human answered "${title}": ${event.answer}`;
      sent = this.#notify(content, { ...meta, event: "answered" });
    } else {
      switch (event.reason) {
        case "deleted":
          sent = this.#notify(`The human dismissed "${title}" without answering.`, {
            ...meta,
            event: "dismissed",
          });
          break;
        case "answered_in_pane":
        case "pane_closed":
          break;
        default:
          event.reason satisfies never;
      }
    }
    sent?.catch((e: unknown) => console.error(`hailr: cannot tell Claude: ${String(e)}`));
  }

  close(): void {
    this.#broker.close();
  }
}
