import { randomUUID } from "node:crypto";

import { closeBroker, connect, createBroker, register } from "./broker.ts";
import { type Notify, type Session, createSession, handleEvent, handleLost } from "./session.ts";

/** broker の出来事を Session に渡し、Session の操作を broker に向ける */
export const startSession = (
  socket: string,
  notify: Notify,
  retryMs?: number,
): { clientId: string; session: Session } => {
  const clientId = randomUUID();
  const session = createSession(
    {
      connect: () => connect(broker),
      register: (q) => register(broker, q),
      close: () => closeBroker(broker),
    },
    notify,
    retryMs,
  );
  const broker = createBroker(socket, clientId, {
    onEvent: (event) => handleEvent(session, event),
    onLost: () => handleLost(session),
  });
  return { clientId, session };
};
