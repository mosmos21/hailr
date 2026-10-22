import type { AskDeps } from "./ask.ts";
import type { PermissionRequest } from "./channel.ts";
import { inHerdrPane } from "./paths.ts";

/** 端末のダイアログでも答えられるので、登録できなければログだけ出して諦める */
export async function askPermission(request: PermissionRequest, deps: AskDeps): Promise<void> {
  const { session, paneId } = deps;
  const leave = (why: string) =>
    console.error(`hailr: permission ${request.request_id} stays in the pane: ${why}`);
  if (!inHerdrPane(paneId)) {
    leave("not inside a herdr pane");
    return;
  }
  if (!session) {
    leave("no hailr socket");
    return;
  }
  try {
    await session.ask({
      id: deps.newId(),
      clientId: session.clientId,
      paneId,
      askedAt: deps.now(),
      body: {
        kind: "permission",
        requestId: request.request_id,
        toolName: request.tool_name,
        description: request.description,
        inputPreview: request.input_preview,
      },
    });
  } catch (e) {
    leave(String(e));
  }
}
