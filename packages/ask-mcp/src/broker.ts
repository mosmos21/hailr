import http from "node:http";

import { type ClientEvent, clientEvent, readSse } from "./events.ts";

/** broker が動いていない、または答えない */
export class BrokerUnavailable extends Error {}

/** broker が質問を断った */
export class Rejected extends Error {}

export type Question = {
  id: string;
  clientId: string;
  paneId: string;
  askedAt: number;
  body:
    | {
        kind: "ask";
        title: string;
        question: string;
        options: string[];
        links: string[];
      }
    | {
        kind: "permission";
        requestId: string;
        toolName: string;
        description: string;
        inputPreview: string;
      };
};

/** 区切りが来ないまま伸び続ける SSE を読み続けないため、1 件の上限を文字数で置く */
const MAX_PENDING_EVENT = 1 << 20;

export type Broker = {
  readonly socketPath: string;
  readonly clientId: string;
  readonly timeoutMs: number;
  /** SSE で出来事が届いたとき */
  readonly onEvent: (event: ClientEvent) => void;
  /** つながっていた SSE が closeBroker() 以外で切れたとき */
  readonly onLost: () => void;
  events: http.ClientRequest | undefined;
  opening: http.ClientRequest | undefined;
  connecting: Promise<void> | undefined;
  closed: boolean;
};

export type BrokerOptions = {
  timeoutMs?: number;
  onEvent?: (event: ClientEvent) => void;
  onLost?: () => void;
};

export const createBroker = (
  socketPath: string,
  clientId: string,
  { timeoutMs = 2000, onEvent = () => {}, onLost = () => {} }: BrokerOptions = {},
): Broker => {
  return {
    socketPath,
    clientId,
    timeoutMs,
    onEvent,
    onLost,
    events: undefined,
    opening: undefined,
    connecting: undefined,
    closed: false,
  };
};

/** broker は SSE がつながっている client の質問しか受け付けないため、登録の前につなぐ。
 * 2 本目の SSE は 1 本目で登録した質問を消すため、同時に呼ばれたら 1 本の接続を待ち合わせる */
export const connect = (broker: Broker): Promise<void> => {
  if (broker.closed) {
    return Promise.reject(new BrokerUnavailable("closed"));
  }
  if (broker.events) {
    return Promise.resolve();
  }
  broker.connecting ??= open(broker).finally(() => {
    broker.connecting = undefined;
  });
  return broker.connecting;
};

const open = (broker: Broker): Promise<void> => {
  return new Promise<void>((resolve, reject) => {
    const request = http.get(
      {
        socketPath: broker.socketPath,
        path: `/clients/${broker.clientId}/events`,
        timeout: broker.timeoutMs,
      },
      (response) => {
        broker.opening = undefined;
        if (response.statusCode !== 200) {
          response.resume();
          request.destroy();
          reject(new BrokerUnavailable(`events: ${response.statusCode}`));
          return;
        }
        if (broker.closed) {
          request.destroy();
          reject(new BrokerUnavailable("closed"));
          return;
        }
        // keep-alive の間隔は期限より長いため、つながったら期限を外す
        request.setTimeout(0);
        broker.events = request;
        response.on("close", () => forget(broker, request));
        let buffer = "";
        response.setEncoding("utf8");
        response.on("data", (chunk: string) => {
          const read = readSse(buffer + chunk);
          buffer = read.rest;
          if (buffer.length > MAX_PENDING_EVENT) {
            console.error("hailr: an event too long to read; disconnected");
            request.destroy();
            return;
          }
          for (const data of read.data) {
            deliver(broker, data);
          }
        });
        resolve();
      },
    );
    broker.opening = request;
    request.on("timeout", () => request.destroy(new Error("timed out")));
    request.on("error", (e) => {
      if (broker.opening === request) {
        broker.opening = undefined;
      }
      forget(broker, request);
      reject(new BrokerUnavailable(e.message));
    });
  });
};

export const register = async (broker: Broker, question: Question): Promise<void> => {
  const events = broker.events;
  const body = JSON.stringify(question);
  const { status, text } = await new Promise<{ status: number; text: string }>(
    (resolve, reject) => {
      const request = http.request(
        {
          socketPath: broker.socketPath,
          path: "/questions",
          method: "POST",
          timeout: broker.timeoutMs,
          headers: {
            "content-type": "application/json",
            "content-length": Buffer.byteLength(body),
          },
        },
        (response) => {
          let text = "";
          response.setEncoding("utf8");
          response.on("data", (chunk: string) => (text += chunk));
          response.on("end", () => resolve({ status: response.statusCode ?? 0, text }));
          response.on("close", () => {
            if (!response.complete) {
              reject(new BrokerUnavailable("the reply was cut off"));
            }
          });
        },
      );
      request.on("timeout", () => request.destroy(new Error("timed out")));
      request.on("error", (e) => reject(new BrokerUnavailable(e.message)));
      request.end(body);
    },
  );
  if (status === 200 || status === 201) {
    return;
  }
  if (status === 400) {
    throw new Rejected(text);
  }
  if (status === 409) {
    // broker はもうこの client を知らないので、次の呼び出しでつなぎ直させる。その間に張り直した接続は切らない
    if (events && broker.events === events) {
      events.destroy();
      forget(broker, events);
    }
    throw new BrokerUnavailable("the broker forgot this client");
  }
  throw new Error(`hailr answered ${status}: ${text}`);
};

/** 開きかけの接続も切り、以後はつながない */
export const closeBroker = (broker: Broker): void => {
  broker.closed = true;
  broker.opening?.destroy();
  broker.opening = undefined;
  broker.events?.destroy();
  broker.events = undefined;
};

const forget = (broker: Broker, request: http.ClientRequest): void => {
  if (broker.events === request) {
    broker.events = undefined;
    broker.onLost();
  }
};

const deliver = (broker: Broker, data: string): void => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    console.error("hailr: an event that is not JSON");
    return;
  }
  const event = clientEvent.safeParse(parsed);
  if (!event.success) {
    console.error("hailr: an event of an unknown shape");
    return;
  }
  broker.onEvent(event.data);
};
