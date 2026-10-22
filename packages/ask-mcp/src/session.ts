import { type Broker, BrokerUnavailable, type Question, Rejected } from "./broker.ts";
import type { ClientEvent } from "./events.ts";

export type Notify = (content: string, meta: Record<string, string>) => Promise<void>;

type SessionBroker = Pick<
  Broker,
  "connect" | "register" | "close" | "clientId" | "onEvent" | "onLost"
>;

/** 未回答の質問を持ち、届いた回答を Claude に返す。broker が再起動しても、つなぎ直して登録し直す */
export class Session {
  readonly #broker: SessionBroker;
  readonly #notify: Notify;
  readonly #retryMs: number;
  readonly #pending = new Map<string, Question>();
  #retry: NodeJS.Timeout | undefined;
  #warned = false;
  #running = false;
  #again = false;
  #closed = false;

  constructor(broker: SessionBroker, notify: Notify, retryMs = 1000) {
    this.#broker = broker;
    this.#notify = notify;
    this.#retryMs = retryMs;
    broker.onEvent = (event) => this.#handle(event);
    broker.onLost = () => this.#lost();
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

  /** 未回答が無ければ次の ask までつながない。送り直しの最中なら、終わってからもう一度送る */
  #lost(): void {
    if (this.#closed || this.#pending.size === 0) {
      return;
    }
    if (this.#running) {
      this.#again = true;
      return;
    }
    if (this.#retry) {
      return;
    }
    this.#retry = setTimeout(() => {
      this.#retry = undefined;
      void this.#reconnect();
    }, this.#retryMs);
  }

  async #reconnect(): Promise<void> {
    if (this.#closed || this.#pending.size === 0) {
      return;
    }
    this.#running = true;
    this.#again = false;
    try {
      if ((await this.#askAgain()) && this.#warned) {
        console.error("hailr: connected again; asked the unanswered questions again");
        this.#warned = false;
      }
    } finally {
      this.#running = false;
    }
    if (this.#again) {
      this.#lost();
    }
  }

  /** 未回答を全部届けられたら true。届かなかったものは次の周で送り直す */
  async #askAgain(): Promise<boolean> {
    try {
      await this.#broker.connect();
    } catch (e) {
      this.#warn(`waiting for hailr to come back: ${String(e)}`);
      this.#again = true;
      return false;
    }
    let done = true;
    for (const question of this.pending()) {
      if (this.#closed) {
        return false;
      }
      if (!this.#pending.has(question.id)) {
        continue;
      }
      try {
        await this.#broker.register(question);
      } catch (e) {
        if (e instanceof Rejected) {
          console.error(`hailr: ${question.id} was refused: ${e.message}`);
          this.#pending.delete(question.id);
        } else if (e instanceof BrokerUnavailable) {
          this.#warn(`${question.id} not asked again yet: ${e.message}`);
          this.#again = true;
          done = false;
        } else {
          // broker の不具合は送り直しても直らないため、知らせるだけにする
          console.error(`hailr: ${question.id} cannot be asked again: ${String(e)}`);
        }
      }
    }
    return done;
  }

  /** 失敗が続く間は最初の 1 回だけ出す */
  #warn(message: string): void {
    if (!this.#warned) {
      console.error(`hailr: ${message}`);
      this.#warned = true;
    }
  }

  close(): void {
    this.#closed = true;
    clearTimeout(this.#retry);
    this.#broker.close();
  }
}
