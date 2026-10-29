import { afterEach, expect, onTestFinished, test, vi } from "vitest";

import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";
import type { ChannelNotification } from "./channel.ts";
import {
  type Notify,
  ask,
  closeSession,
  createSession,
  handleEvent,
  handleLost,
  pending,
} from "./session.ts";
import { permission, question } from "./testing.ts";

const verdict = ({ requestId, behavior }: { requestId: string; behavior: "allow" | "deny" }) => ({
  method: "notifications/claude/channel/permission",
  params: { request_id: requestId, behavior },
});

afterEach(() => {
  vi.useRealTimers();
});

const logged = (log: { mock: { calls: unknown[][] } }, text: string) =>
  log.mock.calls.filter(([m]) => String(m).includes(text)).length;

const setup = () => {
  const calls: string[] = [];
  const notices: ChannelNotification[] = [];
  let connect = async () => {};
  let register = async (_: Question) => {};
  const port = {
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
  const notify: Notify = async (n) => {
    notices.push(n);
    if (fail) throw new Error("stdout closed");
  };
  const session = createSession(port, notify);
  return {
    session,
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
};

test("keeps the question from before it is sent until the answer", async () => {
  const s = setup();
  s.onRegister(async (q) => {
    expect(pending(s.session)).toEqual([q]);
  });
  await ask(s.session, question({ id: "q1" }));
  expect(s.calls).toEqual(["connect", "register q1"]);

  s.onRegister(async () => {
    throw new Rejected("no");
  });
  await expect(ask(s.session, question({ id: "q2" }))).rejects.toBeInstanceOf(Rejected);
  s.onRegister(async () => {
    throw new BrokerUnavailable("the reply was cut off");
  });
  await expect(ask(s.session, question({ id: "q3" }))).rejects.toBeInstanceOf(BrokerUnavailable);
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
});

test("tells Claude about answers and dismissals once", async () => {
  const s = setup();
  for (const id of ["a", "b", "c", "d"]) {
    await ask(s.session, question({ id, title: `T${id}` }));
  }
  handleEvent(s.session, { type: "answered", id: "a", answer: "Yes" });
  handleEvent(s.session, { type: "answered", id: "a", answer: "again" });
  handleEvent(s.session, { type: "withdrawn", id: "b", reason: "deleted" });
  handleEvent(s.session, { type: "withdrawn", id: "c", reason: "answered_in_pane" });
  handleEvent(s.session, { type: "withdrawn", id: "d", reason: "pane_closed" });
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  handleEvent(s.session, { type: "answered", id: "unknown", answer: "x" });
  expect(log).toHaveBeenCalledWith(expect.stringContaining("unknown"));
  expect(s.notices).toEqual([
    {
      method: "notifications/claude/channel",
      params: { content: 'The human answered "Ta": Yes', meta: { question_id: "a", event: "answered" } },
    },
    {
      method: "notifications/claude/channel",
      params: {
        content: 'The human dismissed "Tb" without answering.',
        meta: { question_id: "b", event: "dismissed" },
      },
    },
  ]);
  expect(pending(s.session)).toEqual([]);
});

test("sends an answer that arrives before the broker replies once", async () => {
  const s = setup();
  s.onRegister(async (q) => {
    handleEvent(s.session, { type: "answered", id: q.id, answer: "Yes" });
  });
  await ask(s.session, question({ id: "q1" }));
  expect(s.notices).toHaveLength(1);
  expect(pending(s.session)).toEqual([]);
});

test("logs a notification Claude Code cannot take", async () => {
  const s = setup();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  s.failNotify();
  await ask(s.session, question({ id: "q1" }));
  handleEvent(s.session, { type: "answered", id: "q1", answer: "Yes" });
  await vi.waitFor(() => expect(log).toHaveBeenCalledWith(expect.stringContaining("stdout closed")));
  expect(pending(s.session)).toEqual([]);
});

test("connects again a second after the connection drops, once for repeated drops", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, question({ id: "q1" }));
  s.calls.length = 0;
  handleLost(s.session);
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(999);
  expect(s.calls).toEqual([]);
  await vi.advanceTimersByTimeAsync(1);
  expect(s.calls).toEqual(["connect", "register q1"]);
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toHaveLength(2);
});

test("keeps connecting while hailr is down, warning once per outage", async () => {
  vi.useFakeTimers();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  await ask(s.session, question({ id: "q1" }));
  s.calls.length = 0;

  s.failConnect(true);
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(2000);
  expect(s.calls).toEqual(["connect", "connect"]);
  expect(logged(log, "waiting")).toBe(1);

  s.failConnect(false);
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls.slice(2)).toEqual(["connect", "register q1"]);
  expect(logged(log, "connected again")).toBe(1);

  s.failConnect(true);
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(1000);
  expect(logged(log, "waiting")).toBe(2);
});

test("asks again only what is still unanswered", async () => {
  vi.useFakeTimers();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  for (const id of ["q1", "q2", "q3"]) {
    await ask(s.session, question({ id }));
  }
  s.calls.length = 0;
  s.onRegister(async (q) => {
    if (q.id === "q1") handleEvent(s.session, { type: "answered", id: "q2", answer: "Yes" });
    if (q.id === "q3") throw new Rejected("no longer valid");
  });
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls).toEqual(["connect", "register q1", "register q3"]);
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toHaveLength(3);
});

test("registers again only while hailr cannot be reached, giving up on other errors", async () => {
  vi.useFakeTimers();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  await ask(s.session, question({ id: "q1" }));
  s.calls.length = 0;
  let attempts = 0;
  s.onRegister(async () => {
    attempts++;
    if (attempts <= 2) throw new BrokerUnavailable("timed out");
    throw new Error("hailr answered 500: boom");
  });
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual(["connect", "register q1", "connect", "register q1", "connect", "register q1"]);
  expect(logged(log, "not asked again")).toBe(1);
  expect(log).toHaveBeenCalledWith(expect.stringContaining("cannot be asked again"));
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
});

test("runs one round at a time and stops in the middle on close", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, question({ id: "q1" }));
  await ask(s.session, question({ id: "q2" }));
  s.calls.length = 0;
  let release = () => {};
  s.onRegister(() => new Promise<void>((resolve) => (release = resolve)));
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(1000);
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toEqual(["connect", "register q1"]);
  closeSession(s.session);
  release();
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toEqual(["connect", "register q1", "close"]);
});

test("stops for good when closed while connecting again", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, question({ id: "q1" }));
  s.calls.length = 0;
  const reject = s.holdConnect();
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(1000);
  closeSession(s.session);
  reject(new BrokerUnavailable("closed"));
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual(["connect", "close"]);
});

test("does not connect when the question was answered before the timer fired", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, question({ id: "q1" }));
  s.calls.length = 0;
  handleLost(s.session);
  handleEvent(s.session, { type: "answered", id: "q1", answer: "Yes" });
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls).toEqual([]);
});

test("waits for the next question when nothing is pending, and stops on close", async () => {
  vi.useFakeTimers();
  const s = setup();
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual([]);

  await ask(s.session, question({ id: "q1" }));
  handleLost(s.session);
  closeSession(s.session);
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual(["connect", "register q1", "close"]);
});

test("answers permission requests with allow or deny and nothing else", async () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  const odd = ["Yes", "constructor", "toString", "__proto__"];
  for (const [id, requestId] of [["p1", "abcde"], ["p2", "fghij"], ["p4", "qrstu"]] as const) {
    await ask(s.session, permission({ id, requestId }));
  }
  for (const answer of odd) {
    await ask(s.session, permission({ id: answer, requestId: "kmnop" }));
  }
  handleEvent(s.session, { type: "answered", id: "p1", answer: "Allow" });
  handleEvent(s.session, { type: "answered", id: "p2", answer: "Deny" });
  for (const answer of odd) {
    handleEvent(s.session, { type: "answered", id: answer, answer });
  }
  handleEvent(s.session, { type: "withdrawn", id: "p4", reason: "deleted" });
  expect(s.notices).toEqual([
    verdict({ requestId: "abcde", behavior: "allow" }),
    verdict({ requestId: "fghij", behavior: "deny" }),
  ]);
  expect(log).toHaveBeenCalledWith(expect.stringContaining("neither Allow nor Deny"));
  expect(pending(s.session)).toEqual([]);
});

test("does not bring back permission requests after reconnecting", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, permission({ id: "p1", requestId: "abcde" }));
  await ask(s.session, question({ id: "q1" }));
  s.calls.length = 0;
  handleLost(s.session);
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls).toEqual(["connect", "register q1"]);

  handleEvent(s.session, { type: "answered", id: "q1", answer: "x" });
  await ask(s.session, permission({ id: "p2", requestId: "fghij" }));
  s.calls.length = 0;
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual([]);
});

test("keeps a permission request that arrives while reconnecting", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, question({ id: "q1" }));
  s.calls.length = 0;
  const reject = s.holdConnect();
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(1000);
  const asking = ask(s.session, permission({ id: "p1", requestId: "abcde" }));
  reject(new BrokerUnavailable("ENOENT"));
  await expect(asking).rejects.toBeInstanceOf(BrokerUnavailable);
  s.failConnect(false);
  await ask(s.session, permission({ id: "p2", requestId: "fghij" }));
  await vi.advanceTimersByTimeAsync(1000);
  handleEvent(s.session, { type: "answered", id: "p2", answer: "Allow" });
  expect(s.notices).toEqual([verdict({ requestId: "fghij", behavior: "allow" })]);
});
