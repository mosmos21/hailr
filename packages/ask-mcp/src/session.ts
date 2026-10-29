import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";
import {
  type ChannelNotification,
  channelNotification,
  permissionVerdict,
} from "./channel.ts";
import type { ClientEvent } from "./events.ts";

export type Notify = (notification: ChannelNotification) => Promise<void>;

/** Session が broker に頼む操作 */
export type SessionPort = {
  connect: () => Promise<void>;
  register: (question: Question) => Promise<void>;
  close: () => void;
};

/** 未回答の質問を持ち、届いた回答を Claude に返す。broker が再起動しても、つなぎ直して登録し直す */
export type Session = {
  readonly port: SessionPort;
  readonly notify: Notify;
  readonly retryMs: number;
  readonly questions: Map<string, Question>;
  retry: NodeJS.Timeout | undefined;
  warned: boolean;
  running: boolean;
  again: boolean;
  closed: boolean;
};

export const createSession = (port: SessionPort, notify: Notify, retryMs = 1000): Session => {
  return {
    port,
    notify,
    retryMs,
    questions: new Map(),
    retry: undefined,
    warned: false,
    running: false,
    again: false,
    closed: false,
  };
};

export const pending = (session: Session): Question[] => {
  return [...session.questions.values()];
};

/** 登録の直後に届く回答を取りこぼさないよう、送る前に手元に置く。
 * 失敗は呼び出し元が Claude に返すか pane に任せるので、broker に届いていても手元からは消す */
export const ask = async (session: Session, question: Question): Promise<void> => {
  session.questions.set(question.id, question);
  try {
    await session.port.connect();
    await session.port.register(question);
  } catch (e) {
    session.questions.delete(question.id);
    throw e;
  }
};

/** SSE で届いた出来事を、手元の質問に照らして Claude に返す */
export const handleEvent = (session: Session, event: ClientEvent): void => {
  const question = session.questions.get(event.id);
  if (!question) {
    console.error(`hailr: an event for a question this session does not hold: ${event.id}`);
    return;
  }
  session.questions.delete(event.id);
  const sent = reply(session, question, event);
  sent?.catch((e: unknown) => console.error(`hailr: cannot tell Claude: ${String(e)}`));
};

/** SSE が切れたとき */
export const handleLost = (session: Session): void => {
  forgetPermissions(session);
  lost(session);
};

export const closeSession = (session: Session): void => {
  session.closed = true;
  clearTimeout(session.retry);
  session.port.close();
};

const reply = (session: Session, question: Question, event: ClientEvent): Promise<void> | undefined => {
  if (question.body.kind === "permission") {
    // Claude Code には権限確認の取り下げを受ける口が無いため、返さない
    if (event.type !== "answered") {
      return undefined;
    }
    const verdict =
      event.answer === "Allow" ? "allow" : event.answer === "Deny" ? "deny" : undefined;
    if (!verdict) {
      console.error(`hailr: ${event.id} was answered with neither Allow nor Deny`);
      return undefined;
    }
    return session.notify(permissionVerdict(question.body.requestId, verdict));
  }
  const title = question.body.title;
  const meta = { question_id: event.id };
  let sent: Promise<void> | undefined;
  if (event.type === "answered") {
    const content = `The human answered "${title}": ${event.answer}`;
    sent = session.notify(channelNotification(content, { ...meta, event: "answered" }));
  } else {
    switch (event.reason) {
      case "deleted":
        sent = session.notify(
          channelNotification(`The human dismissed "${title}" without answering.`, {
            ...meta,
            event: "dismissed",
          }),
        );
        break;
      case "answered_in_pane":
      case "pane_closed":
        break;
      default:
        event.reason satisfies never;
    }
  }
  return sent;
};

/** 未回答が無ければ次の ask までつながない。送り直しの最中なら、終わってからもう一度送る */
const lost = (session: Session): void => {
  if (session.closed || session.questions.size === 0) {
    return;
  }
  if (session.running) {
    session.again = true;
    return;
  }
  if (session.retry) {
    return;
  }
  session.retry = setTimeout(() => {
    session.retry = undefined;
    void reconnect(session);
  }, session.retryMs);
};

const reconnect = async (session: Session): Promise<void> => {
  if (session.closed || session.questions.size === 0) {
    return;
  }
  session.running = true;
  session.again = false;
  try {
    if ((await askAgain(session)) && session.warned) {
      console.error("hailr: connected again; asked the unanswered questions again");
      session.warned = false;
    }
  } finally {
    session.running = false;
  }
  if (session.again) {
    lost(session);
  }
};

/** 未回答を全部届けられたら true。届かなかったものは次の周で送り直す */
const askAgain = async (session: Session): Promise<boolean> => {
  try {
    await session.port.connect();
  } catch (e) {
    warn(session, `waiting for hailr to come back: ${String(e)}`);
    session.again = true;
    return false;
  }
  let done = true;
  for (const question of pending(session)) {
    if (session.closed) {
      return false;
    }
    if (!session.questions.has(question.id)) {
      continue;
    }
    try {
      await session.port.register(question);
    } catch (e) {
      if (e instanceof Rejected) {
        console.error(`hailr: ${question.id} was refused: ${e.message}`);
        session.questions.delete(question.id);
      } else if (e instanceof BrokerUnavailable) {
        warn(session, `${question.id} not asked again yet: ${e.message}`);
        session.again = true;
        done = false;
      } else {
        // broker の不具合は送り直しても直らないため、知らせるだけにする
        console.error(`hailr: ${question.id} cannot be asked again: ${String(e)}`);
      }
    }
  }
  return done;
};

/** 端末で答え済みの権限確認は知らせが来ないため、切れた時点のものは古いカードとして出し直さない */
const forgetPermissions = (session: Session): void => {
  for (const question of pending(session)) {
    if (question.body.kind === "permission") {
      session.questions.delete(question.id);
    }
  }
};

/** 失敗が続く間は最初の 1 回だけ出す */
const warn = (session: Session, message: string): void => {
  if (!session.warned) {
    console.error(`hailr: ${message}`);
    session.warned = true;
  }
};
