import { expect, onTestFinished, test, vi } from "vitest";

import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";
import type { ClientEvent } from "./events.ts";
import { type Notify, Session } from "./session.ts";

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
  let register = async (_: Question) => {};
  const broker = {
    clientId: "c1",
    onEvent: (_: ClientEvent) => {},
    connect: () => {
      calls.push("connect");
      return Promise.resolve();
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
