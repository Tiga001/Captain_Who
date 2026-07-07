// Renderer UI.
import { Globe2 } from "lucide-react";
import type { AgentToolCall, AgentToolResult } from "@mycopilot/protocol";
import { useFrontendConfig } from "../../../../config/FrontendConfigProvider";
import type { Translate } from "../../../../config/translationFormat";
import type { ChatWebSearchActivity } from "../../chatTypes";
import { AgentActivityDisclosure } from "./AgentActivityDisclosure";
import { WebSearchSourcesList } from "./WebSearchSources";

interface WebSearchToolActivityProps {
  activity?: ChatWebSearchActivity;
  call: AgentToolCall;
  result?: AgentToolResult;
}

function getWebActivityKind(call: AgentToolCall, activity: ChatWebSearchActivity | undefined) {
  return activity?.kind ?? (call.tool === "web_fetch" ? "fetch" : "search");
}

function getWebActivityStatusLabel(
  t: Translate,
  kind: ChatWebSearchActivity["kind"],
  activity: ChatWebSearchActivity | undefined,
  result?: AgentToolResult,
) {
  if (kind === "fetch") {
    if (activity?.status === "cancelled") return t("agent.web.fetch.cancelled");
    if (activity?.status === "failed" || result?.ok === false) return t("agent.web.fetch.failed");
    if (activity?.status === "completed" || result) return t("agent.web.fetch.completed");
    return t("agent.web.fetch.running");
  }

  if (activity?.status === "cancelled") return t("agent.web.search.cancelled");
  if (activity?.status === "failed" || result?.ok === false) return t("agent.web.search.failed");
  if (activity?.status === "completed" || result) return t("agent.web.search.completed");
  return t("agent.web.search.running");
}

export function WebSearchToolActivity({ activity, call, result }: WebSearchToolActivityProps) {
  const { t } = useFrontendConfig();
  const kind = getWebActivityKind(call, activity);
  const hasDetails = Boolean(
    activity?.query ||
      activity?.sources.length ||
      activity?.answer ||
      activity?.error ||
      result?.error,
  );
  const label = getWebActivityStatusLabel(t, kind, activity, result);
  const isPending =
    !result &&
    activity?.status !== "completed" &&
    activity?.status !== "failed" &&
    activity?.status !== "cancelled";
  const queryLabel = kind === "fetch" ? t("agent.web.query.fetch") : t("agent.web.query.search");

  return (
    <AgentActivityDisclosure
      className="agent-activity--web-search"
      hasDetails={hasDetails}
      icon={Globe2}
      isPending={isPending}
      label={label}
    >
      {hasDetails && (
        <div className="agent-activity__details web-search-activity__details">
          {activity?.query && (
            <p className="web-search-activity__query">
              <span>{queryLabel}</span>
              {activity.query}
            </p>
          )}
          {activity?.error || result?.error ? (
            <>
              <span>{t("agent.detail.error")}</span>
              <pre>{activity?.error ?? result?.error}</pre>
            </>
          ) : null}
          {activity?.sources && <WebSearchSourcesList sources={activity.sources} />}
          {activity?.answer && (
            <p className="web-search-activity__answer">
              <span>{t("agent.detail.summary")}</span>
              {activity.answer}
            </p>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  );
}
