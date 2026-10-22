import { afterEach, expect, onTestFinished, test, vi } from "vitest";

import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";
import type { ClientEvent } from "./events.ts";
import { type Notify, Session } from "./session.ts";

afterEach(() => {
  vi.useRealTimers();
});

function question(id: string, title = "Merge?"): Question {
  return {
    id,
    clientId: "c1",
    paneId: "w1:p1",
    askedAt: 1,
    body: { kind: "ask", title, question: "q", options: [], links: [] },
  };
}

function setup() {
  const calls: string[] = [];
  const notices: [string, Record<string, string>][] = [];
  let connect = async () => {};
  let register = async (_: Question) => {};
  const broker = {
    clientId: "c1",
    onEvent: (_: ClientEvent) => {},
    onLost: () => {},
    connect: () => {
      calls.push("connect");
      return connect();
    },
    register: (q: Question) => {
      calls.push(`register ${q.id}`);
      return register(q);
    },
    close: () => calls.push("close"),
  };
  let fail = false;
  const notify: Notify = async (content, meta) => {
    notices.push([content, meta]);
    if (fail) throw new Error("stdout closed");
  };
  const session = new Session(broker, notify);
  return {
    session,
    broker,
    calls,
    notices,
    onRegister: (f: (q: Question) => Promise<void>) => {
      register = f;
    },
    holdConnect: () => {
      let reject = (_: Error) => {};
      connect = () => new Promise((_, r) => (reject = r));
      return (e: Error) => reject(e);
    },
    failConnect: (fail: boolean) => {
      connect = async () => {
        if (fail) throw new BrokerUnavailable("ENOENT");
      };
    },
    failNotify: () => {
      fail = true;
    },
  };
}

test("keeps the question from before it is sent until the answer", async () => {
  const s = setup();
  s.onRegister(async (q) => {
    expect(s.session.pending()).toEqual([q]);
  });
  await s.session.ask(question("q1"));
  expect(s.calls).toEqual(["connect", "register q1"]);

  s.onRegister(async () => {
    throw new Rejected("no");
  });
  await expect(s.session.ask(question("q2"))).rejects.toBeInstanceOf(Rejected);
  s.onRegister(async () => {
    throw new BrokerUnavailable("the reply was cut off");
  });
  await expect(s.session.ask(question("q3"))).rejects.toBeInstanceOf(BrokerUnavailable);
  expect(s.session.pending().map((q) => q.id)).toEqual(["q1"]);
});

test("tells Claude about answers and dismissals once", async () => {
  const s = setup();
  for (const id of ["a", "b", "c", "d"]) {
    await s.session.ask(question(id, `T${id}`));
  }
  s.broker.onEvent({ type: "answered", id: "a", answer: "Yes" });
  s.broker.onEvent({ type: "answered", id: "a", answer: "again" });
  s.broker.onEvent({ type: "withdrawn", id: "b", reason: "deleted" });
  s.broker.onEvent({ type: "withdrawn", id: "c", reason: "answered_in_pane" });
  s.broker.onEvent({ type: "withdrawn", id: "d", reason: "pane_closed" });
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  s.broker.onEvent({ type: "answered", id: "unknown", answer: "x" });
  expect(log).toHaveBeenCalledWith(expect.stringContaining("unknown"));
  expect(s.notices).toEqual([
    ['The human answered "Ta": Yes', { question_id: "a", event: "answered" }],
    ['The human dismissed "Tb" without answering.', { question_id: "b", event: "dismissed" }],
  ]);
  expect(s.session.pending()).toEqual([]);
});

test("sends an answer that arrives before the broker replies once", async () => {
  const s = setup();
  s.onRegister(async (q) => {
    s.broker.onEvent({ type: "answered", id: q.id, answer: "Yes" });
  });
  await s.session.ask(question("q1"));
  expect(s.notices).toHaveLength(1);
  expect(s.session.pending()).toEqual([]);
});

test("logs a notification Claude Code cannot take", async () => {
  const s = setup();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  s.failNotify();
  await s.session.ask(question("q1"));
  s.broker.onEvent({ type: "answered", id: "q1", answer: "Yes" });
  await vi.waitFor(() => expect(log).toHaveBeenCalledWith(expect.stringContaining("stdout closed")));
  expect(s.session.pending()).toEqual([]);
});

test("connects again and asks the unanswered questions again", async () => {
  vi.useFakeTimers();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  for (const id of ["q1", "q2", "q3"]) {
    await s.session.ask(question(id));
  }
  s.calls.length = 0;

  s.failConnect(true);
  s.broker.onLost();
  s.broker.onLost();
  await vi.advanceTimersByTimeAsync(999);
  expect(s.calls).toEqual([]);
  await vi.advanceTimersByTimeAsync(1);
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls).toEqual(["connect", "connect"]);
  const waiting = () => log.mock.calls.filter(([m]) => String(m).includes("waiting")).length;
  expect(waiting()).toBe(1);

  s.failConnect(false);
  s.onRegister(async (q) => {
    if (q.id === "q1") s.broker.onEvent({ type: "answered", id: "q2", answer: "Yes" });
    if (q.id === "q3") throw new Rejected("no longer valid");
  });
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls.slice(2)).toEqual(["connect", "register q1", "register q3"]);
  expect(s.session.pending().map((q) => q.id)).toEqual(["q1"]);
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toHaveLength(5);

  s.failConnect(true);
  s.broker.onLost();
  await vi.advanceTimersByTimeAsync(1000);
  expect(waiting()).toBe(2);
});

test("asks again only while hailr cannot be reached, logging once", async () => {
  vi.useFakeTimers();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  await s.session.ask(question("q1"));
  s.calls.length = 0;
  let attempts = 0;
  s.onRegister(async () => {
    attempts++;
    if (attempts <= 2) throw new BrokerUnavailable("timed out");
    throw new Error("hailr answered 500: boom");
  });
  s.broker.onLost();
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual(["connect", "register q1", "connect", "register q1", "connect", "register q1"]);
  expect(log.mock.calls.filter(([m]) => String(m).includes("not asked again"))).toHaveLength(1);
  expect(log).toHaveBeenCalledWith(expect.stringContaining("cannot be asked again"));
  expect(s.session.pending().map((q) => q.id)).toEqual(["q1"]);
});

test("runs one round at a time and stops in the middle on close", async () => {
  vi.useFakeTimers();
  const s = setup();
  await s.session.ask(question("q1"));
  await s.session.ask(question("q2"));
  s.calls.length = 0;
  let release = () => {};
  s.onRegister(() => new Promise<void>((resolve) => (release = resolve)));
  s.broker.onLost();
  await vi.advanceTimersByTimeAsync(1000);
  s.broker.onLost();
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toEqual(["connect", "register q1"]);
  s.session.close();
  release();
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toEqual(["connect", "register q1", "close"]);
});

test("stops for good when closed while connecting again", async () => {
  vi.useFakeTimers();
  const s = setup();
  await s.session.ask(question("q1"));
  s.calls.length = 0;
  const reject = s.holdConnect();
  s.broker.onLost();
  await vi.advanceTimersByTimeAsync(1000);
  s.session.close();
  reject(new BrokerUnavailable("closed"));
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual(["connect", "close"]);
});

test("does not connect when the question was answered before the timer fired", async () => {
  vi.useFakeTimers();
  const s = setup();
  await s.session.ask(question("q1"));
  s.calls.length = 0;
  s.broker.onLost();
  s.broker.onEvent({ type: "answered", id: "q1", answer: "Yes" });
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls).toEqual([]);
});

test("waits for the next question when nothing is pending, and stops on close", async () => {
  vi.useFakeTimers();
  const s = setup();
  s.broker.onLost();
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual([]);

  await s.session.ask(question("q1"));
  s.broker.onLost();
  s.session.close();
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual(["connect", "register q1", "close"]);
});
