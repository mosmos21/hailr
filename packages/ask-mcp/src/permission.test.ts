import { expect, onTestFinished, test, vi } from "vitest";

import { BrokerUnavailable, type Question } from "./broker.ts";
import { askPermission } from "./permission.ts";
import { askDeps, fixture } from "./testing.ts";

const expected = fixture("question-permission.json") as Question;

test("asks a permission request the way the protocol fixture shows", async () => {
  const { value, asked } = askDeps({ expected });
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
  const down = askDeps({
    expected,
    ask: async () => {
      throw new BrokerUnavailable("ENOENT");
    },
  });
  await askPermission(request, down.value);
  for (const paneId of [undefined, "", "-x"]) {
    await askPermission(request, { ...down.value, paneId });
  }
  await askPermission(request, { ...down.value, asker: undefined });
  expect(down.asked).toHaveLength(1);
  expect(log).toHaveBeenCalledTimes(5);
  expect(log.mock.calls.flat().join(" ")).not.toContain("secret");
});
