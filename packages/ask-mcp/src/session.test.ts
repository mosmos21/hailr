import { afterEach, expect, onTestFinished, test, vi } from "vitest";

import { BrokerUnavailable, type Question, Rejected } from "./broker.ts";
import { type ChannelNotification, channelNotification, permissionVerdict } from "./channel.ts";
import {
  type Notify,
  ask,
  closeSession,
  createSession,
  handleEvent,
  handleLost,
  pending,
} from "./session.ts";

afterEach(() => {
  vi.useRealTimers();
});

const question = (id: string, title = "Merge?"): Question => {
  return {
    id,
    clientId: "c1",
    paneId: "w1:p1",
    askedAt: 1,
    body: { kind: "ask", title, question: "q", options: [], links: [] },
  };
};

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
  await ask(s.session, question("q1"));
  expect(s.calls).toEqual(["connect", "register q1"]);

  s.onRegister(async () => {
    throw new Rejected("no");
  });
  await expect(ask(s.session, question("q2"))).rejects.toBeInstanceOf(Rejected);
  s.onRegister(async () => {
    throw new BrokerUnavailable("the reply was cut off");
  });
  await expect(ask(s.session, question("q3"))).rejects.toBeInstanceOf(BrokerUnavailable);
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
});

test("tells Claude about answers and dismissals once", async () => {
  const s = setup();
  for (const id of ["a", "b", "c", "d"]) {
    await ask(s.session, question(id, `T${id}`));
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
    channelNotification('The human answered "Ta": Yes', { question_id: "a", event: "answered" }),
    channelNotification('The human dismissed "Tb" without answering.', {
      question_id: "b",
      event: "dismissed",
    }),
  ]);
  expect(pending(s.session)).toEqual([]);
});

test("sends an answer that arrives before the broker replies once", async () => {
  const s = setup();
  s.onRegister(async (q) => {
    handleEvent(s.session, { type: "answered", id: q.id, answer: "Yes" });
  });
  await ask(s.session, question("q1"));
  expect(s.notices).toHaveLength(1);
  expect(pending(s.session)).toEqual([]);
});

test("logs a notification Claude Code cannot take", async () => {
  const s = setup();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  s.failNotify();
  await ask(s.session, question("q1"));
  handleEvent(s.session, { type: "answered", id: "q1", answer: "Yes" });
  await vi.waitFor(() => expect(log).toHaveBeenCalledWith(expect.stringContaining("stdout closed")));
  expect(pending(s.session)).toEqual([]);
});

test("connects again and asks the unanswered questions again", async () => {
  vi.useFakeTimers();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  for (const id of ["q1", "q2", "q3"]) {
    await ask(s.session, question(id));
  }
  s.calls.length = 0;

  s.failConnect(true);
  handleLost(s.session);
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(999);
  expect(s.calls).toEqual([]);
  await vi.advanceTimersByTimeAsync(1);
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls).toEqual(["connect", "connect"]);
  const waiting = () => log.mock.calls.filter(([m]) => String(m).includes("waiting")).length;
  expect(waiting()).toBe(1);

  s.failConnect(false);
  s.onRegister(async (q) => {
    if (q.id === "q1") handleEvent(s.session, { type: "answered", id: "q2", answer: "Yes" });
    if (q.id === "q3") throw new Rejected("no longer valid");
  });
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls.slice(2)).toEqual(["connect", "register q1", "register q3"]);
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
  await vi.advanceTimersByTimeAsync(3000);
  expect(s.calls).toHaveLength(5);

  s.failConnect(true);
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(1000);
  expect(waiting()).toBe(2);
});

test("asks again only while hailr cannot be reached, logging once", async () => {
  vi.useFakeTimers();
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  await ask(s.session, question("q1"));
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
  expect(log.mock.calls.filter(([m]) => String(m).includes("not asked again"))).toHaveLength(1);
  expect(log).toHaveBeenCalledWith(expect.stringContaining("cannot be asked again"));
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
});

test("runs one round at a time and stops in the middle on close", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, question("q1"));
  await ask(s.session, question("q2"));
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
  await ask(s.session, question("q1"));
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
  await ask(s.session, question("q1"));
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

  await ask(s.session, question("q1"));
  handleLost(s.session);
  closeSession(s.session);
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual(["connect", "register q1", "close"]);
});

const permission = (id: string, requestId: string): Question => {
  return {
    ...question(id),
    body: { kind: "permission", requestId, toolName: "Bash", description: "d", inputPreview: "p" },
  };
};

test("answers permission requests with allow or deny and nothing else", async () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const s = setup();
  const odd = ["Yes", "constructor", "toString", "__proto__"];
  for (const [id, requestId] of [["p1", "abcde"], ["p2", "fghij"], ["p4", "qrstu"]] as const) {
    await ask(s.session, permission(id, requestId));
  }
  for (const answer of odd) {
    await ask(s.session, permission(answer, "kmnop"));
  }
  handleEvent(s.session, { type: "answered", id: "p1", answer: "Allow" });
  handleEvent(s.session, { type: "answered", id: "p2", answer: "Deny" });
  for (const answer of odd) {
    handleEvent(s.session, { type: "answered", id: answer, answer });
  }
  handleEvent(s.session, { type: "withdrawn", id: "p4", reason: "deleted" });
  expect(s.notices).toEqual([permissionVerdict("abcde", "allow"), permissionVerdict("fghij", "deny")]);
  expect(log).toHaveBeenCalledWith(expect.stringContaining("neither Allow nor Deny"));
  expect(pending(s.session)).toEqual([]);
});

test("does not bring back permission requests after reconnecting", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, permission("p1", "abcde"));
  await ask(s.session, question("q1"));
  s.calls.length = 0;
  handleLost(s.session);
  expect(pending(s.session).map((q) => q.id)).toEqual(["q1"]);
  await vi.advanceTimersByTimeAsync(1000);
  expect(s.calls).toEqual(["connect", "register q1"]);

  handleEvent(s.session, { type: "answered", id: "q1", answer: "x" });
  await ask(s.session, permission("p2", "fghij"));
  s.calls.length = 0;
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(5000);
  expect(s.calls).toEqual([]);
});

test("keeps a permission request that arrives while reconnecting", async () => {
  vi.useFakeTimers();
  const s = setup();
  await ask(s.session, question("q1"));
  s.calls.length = 0;
  const reject = s.holdConnect();
  handleLost(s.session);
  await vi.advanceTimersByTimeAsync(1000);
  const asking = ask(s.session, permission("p1", "abcde"));
  reject(new BrokerUnavailable("ENOENT"));
  await expect(asking).rejects.toBeInstanceOf(BrokerUnavailable);
  s.failConnect(false);
  await ask(s.session, permission("p2", "fghij"));
  await vi.advanceTimersByTimeAsync(1000);
  handleEvent(s.session, { type: "answered", id: "p2", answer: "Allow" });
  expect(s.notices).toEqual([permissionVerdict("fghij", "allow")]);
});
