import { readFileSync } from "node:fs";

import { expect, onTestFinished, test, vi } from "vitest";

import type { AskDeps } from "./ask.ts";
import { BrokerUnavailable, type Question } from "./broker.ts";
import { askPermission } from "./permission.ts";

function fixture(): Question {
  const url = new URL("../../../fixtures/protocol/question-permission.json", import.meta.url);
  return JSON.parse(readFileSync(url, "utf8")) as Question;
}

function deps(ask: (q: Question) => Promise<void> = async () => {}) {
  const asked: Question[] = [];
  const expected = fixture();
  const value: AskDeps = {
    session: {
      clientId: expected.clientId,
      ask: async (q) => {
        asked.push(q);
        await ask(q);
      },
    },
    paneId: expected.paneId,
    now: () => expected.askedAt,
    newId: () => expected.id,
  };
  return { value, asked, expected };
}

test("asks a permission request the way the protocol fixture shows", async () => {
  const { value, asked, expected } = deps();
  if (expected.body.kind !== "permission") throw new Error("fixture is not a permission");
  const { requestId, toolName, description, inputPreview } = expected.body;
  await askPermission(
    { request_id: requestId, tool_name: toolName, description, input_preview: inputPreview },
    value,
  );
  expect(asked).toEqual([expected]);
});

test("leaves the request to the pane when hailr cannot take it", async () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  onTestFinished(() => log.mockRestore());
  const request = { request_id: "abcde", tool_name: "Bash", description: "d", input_preview: "secret" };
  const down = deps(async () => {
    throw new BrokerUnavailable("ENOENT");
  });
  await askPermission(request, down.value);
  for (const paneId of [undefined, "", "-x"]) {
    await askPermission(request, { ...down.value, paneId });
  }
  await askPermission(request, { ...down.value, session: undefined });
  expect(down.asked).toHaveLength(1);
  expect(log).toHaveBeenCalledTimes(5);
  expect(log.mock.calls.flat().join(" ")).not.toContain("secret");
});
