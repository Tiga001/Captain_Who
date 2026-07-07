// Renderer UI.
import { CheckCircle2, SquareTerminal, XCircle } from "lucide-react";
import type { AgentToolCall, AgentToolResult } from "@mycopilot/protocol";
import { useFrontendConfig } from "../../../../config/FrontendConfigProvider";
import { AgentActivityDisclosure } from "./AgentActivityDisclosure";
import { formatToolDetails, getToolCallLabel } from "./toolActivityUtils";

interface GenericToolActivityProps {
  cancelled?: boolean;
  call: AgentToolCall;
  result?: AgentToolResult;
}

export function GenericToolActivity({ cancelled = false, call, result }: GenericToolActivityProps) {
  const { t } = useFrontendConfig();
  const hasDetails = Boolean(call.args !== undefined || call.reason || result);
  const Icon = result?.ok === false ? XCircle : result ? CheckCircle2 : SquareTerminal;
  const isPending = !result && !cancelled;

  return (
    <AgentActivityDisclosure
      hasDetails={hasDetails}
      icon={Icon}
      isPending={isPending}
      label={getToolCallLabel(call, result, t, { cancelled })}
    >
      {hasDetails && (
        <div className="agent-activity__details">
          {call.reason && <p>{call.reason}</p>}
          {call.args !== undefined && (
            <>
              <span>{t("agent.detail.args")}</span>
              <pre>{formatToolDetails(call.args)}</pre>
            </>
          )}
          {result?.error && (
            <>
              <span>{t("agent.detail.error")}</span>
              <pre>{result.error}</pre>
            </>
          )}
          {result?.result !== undefined && (
            <>
              <span>{t("agent.detail.result")}</span>
              <pre>{formatToolDetails(result.result)}</pre>
            </>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  );
}
