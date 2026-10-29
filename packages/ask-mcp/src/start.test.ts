import { expect, onTestFinished, test, vi } from "vitest";

import type { ChannelNotification } from "./channel.ts";
import { ask, closeSession } from "./session.ts";
import { startSession } from "./start.ts";
import { fakeBroker, question } from "./testing.ts";

test("hands broker events to the session and asks again after the connection drops", async () => {
  const fake = await fakeBroker();
  const notices: ChannelNotification[] = [];
  const { clientId, session } = startSession(fake.socket, async (n) => void notices.push(n), 10);
  onTestFinished(() => closeSession(session));

  const asked = question(clientId);
  await ask(session, asked);
  expect(fake.received).toEqual([asked]);

  fake.server.closeAllConnections();
  await vi.waitFor(() => expect(fake.received).toEqual([asked, asked]));

  fake.sse.get(clientId)?.write(`data: {"type":"answered","id":"${asked.id}","answer":"Yes"}\n\n`);
  await vi.waitFor(() =>
    expect(notices).toEqual([
      {
        method: "notifications/claude/channel",
        params: { content: 'The human answered "t": Yes', meta: { question_id: asked.id, event: "answered" } },
      },
    ]),
  );

  closeSession(session);
  await vi.waitFor(() => expect(fake.streams.get(clientId)).toBe(0));
});
