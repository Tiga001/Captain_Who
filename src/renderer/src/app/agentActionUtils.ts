// Renderer UI.
import type { AgentProposedAction } from "@mycopilot/protocol";

export function getAgentActionId(action: AgentProposedAction) {
  if (action.type === "diff") return action.diff.id;
  if (action.type === "command") return action.command.id;
  return action.call.id;
}

export function getErrorMessage(error: unknown) {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "请求失败，请检查 API 配置后重试。";
}
