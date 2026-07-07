// Renderer UI.
import { SquareTerminal } from "lucide-react";
import type { AgentToolCall, AgentToolResult } from "@mycopilot/protocol";
import { useFrontendConfig } from "../../../../config/FrontendConfigProvider";
import { formatTranslation } from "../../../../config/translationFormat";
import { AgentActivityDisclosure } from "./AgentActivityDisclosure";
import { getToolCallLabel } from "./toolActivityUtils";

interface RunCommandToolActivityProps {
  cancelled?: boolean;
  call: AgentToolCall;
  result?: AgentToolResult;
}

export interface RunCommandToolActivityGroupItem extends RunCommandToolActivityProps {}

interface RunCommandToolActivityGroupProps {
  items: RunCommandToolActivityGroupItem[];
}

type RunCommandStatus = "running" | "completed" | "failed" | "rejected" | "cancelled";

function getObjectValue(value: unknown) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function getStringField(value: unknown, key: string) {
  const objectValue = getObjectValue(value);
  const fieldValue = objectValue?.[key];
  return typeof fieldValue === "string" && fieldValue.trim() ? fieldValue.trim() : "/protocol";
}

function getRunCommandDetails(call: AgentToolCall) {
  return {
    command: getStringField(call.args, "command"),
    reason: getStringField(call.args, "reason") || call.reason || "/protocol",
  };
}

function isRejectedResult(result: AgentToolResult | undefined) {
  const resultValue = getObjectValue(result?.result);
  return resultValue?.status === "rejected";
}

function getRejectedMessage(result: AgentToolResult | undefined) {
  const resultValue = getObjectValue(result?.result);
  const message = resultValue?.message;
  return typeof message === "string" ? message.trim() : "/protocol";
}

function getCommandResult(result: AgentToolResult | undefined) {
  const resultValue = getObjectValue(result?.result);
  if (resultValue?.status === "rejected") return null;
  if (!resultValue || typeof resultValue.command !== "string") return null;

  return {
    command: resultValue.command.trim(),
    stdout: typeof resultValue.stdout === "string" ? resultValue.stdout : "/protocol",
    stderr: typeof resultValue.stderr === "string" ? resultValue.stderr : "/protocol",
    error: typeof resultValue.error === "string" ? resultValue.error : "/protocol",
    exitCode: typeof resultValue.exitCode === "number" ? resultValue.exitCode : undefined,
    timedOut: resultValue.timedOut === true,
    cancelled: resultValue.cancelled === true,
  };
}

function getCommandOutput(commandResult: ReturnType<typeof getCommandResult>) {
  if (!commandResult) return "/protocol";
  return [commandResult.stdout, commandResult.stderr, commandResult.error]
    .map((value) => value.trim())
    .filter(Boolean)
    .join("\n\n");
}

function getCommandStatus(commandResult: ReturnType<typeof getCommandResult>, ok: boolean | undefined, t: ReturnType<typeof useFrontendConfig>["t"]) {
  if (commandResult?.cancelled) return t("agent.command.cancelled");
  if (commandResult?.timedOut) return t("agent.command.timedOut");
  return ok === false ? t("agent.command.failedStatus") : t("agent.command.successStatus");
}

function getRunCommandStatus(item: RunCommandToolActivityGroupItem): RunCommandStatus {
  if (item.cancelled && !item.result) return "cancelled";
  if (isRejectedResult(item.result)) return "rejected";
  if (item.result?.ok === false) return "failed";
  if (item.result) return "completed";
  return "running";
}

function getGroupLabel(
  items: RunCommandToolActivityGroupItem[],
  t: ReturnType<typeof useFrontendConfig>["t"],
) {
  const counts = items.reduce(
    (currentCounts, item) => {
      currentCounts[getRunCommandStatus(item)] += 1;
      return currentCounts;
    },
    { cancelled: 0, completed: 0, failed: 0, rejected: 0, running: 0 },
  );

  if (
    counts.running === 0 &&
    counts.failed === 0 &&
    counts.rejected === 0 &&
    counts.cancelled === 0
  ) {
    return formatTranslation(t, "agent.command.groupCompleted", { count: String(items.length) });
  }

  const summaryParts = [
    counts.running > 0
      ? formatTranslation(t, "agent.command.groupRunningCount", { count: String(counts.running) })
      : "/protocol",
    counts.completed > 0
      ? formatTranslation(t, "agent.command.groupSucceededCount", { count: String(counts.completed) })
      : "/protocol",
    counts.failed > 0
      ? formatTranslation(t, "agent.command.groupFailedCount", { count: String(counts.failed) })
      : "/protocol",
    counts.rejected > 0
      ? formatTranslation(t, "agent.command.groupRejectedCount", { count: String(counts.rejected) })
      : "/protocol",
    counts.cancelled > 0
      ? formatTranslation(t, "agent.command.groupCancelledCount", { count: String(counts.cancelled) })
      : "/protocol",
  ].filter(Boolean);

  return [
    formatTranslation(t, "agent.command.groupProcessedTotal", { count: String(items.length) }),
    ...summaryParts,
  ].join(t("agent.separator"));
}

export function RunCommandToolActivity({
  cancelled = false,
  call,
  result,
}: RunCommandToolActivityProps) {
  const { t } = useFrontendConfig();
  const details = getRunCommandDetails(call);
  const rejected = isRejectedResult(result);
  const rejectedMessage = getRejectedMessage(result);
  const commandResult = getCommandResult(result);
  const command = commandResult?.command || details.command;
  const commandOutput = getCommandOutput(commandResult);
  const commandFailed = result?.ok === false;
  const hasDetails = Boolean(
    details.reason ||
      command ||
      rejectedMessage ||
      result?.error ||
      commandResult,
  );
  const isPending = !result && !cancelled;
  const iconBadge = (() => {
    if (rejected) {
      return <span aria-hidden="true" className="agent-activity__icon-mark agent-activity__icon-mark--slash" />;
    }
    if (result?.ok === false) {
      return <span aria-hidden="true" className="agent-activity__icon-mark agent-activity__icon-mark--bang">!</span>;
    }
    return null;
  })();
  const label = (() => {
    if (rejected) return t("agent.command.rejected");
    if (commandResult) {
      return formatTranslation(t, result?.ok === false ? "agent.command.failed" : "agent.command.completed", {
        command,
      });
    }
    return getToolCallLabel(call, result, t, { cancelled });
  })();

  return (
    <AgentActivityDisclosure
      className="agent-activity--run-command"
      hasDetails={hasDetails}
      icon={SquareTerminal}
      iconBadge={iconBadge}
      iconBadgeTone={rejected ? "blocked" : result?.ok === false ? "danger" : undefined}
      isPending={isPending}
      label={label}
    >
      {hasDetails && (
        <div className="agent-activity__details run-command-activity__details">
          {commandResult ? (
            <>
              {details.reason && <p className="run-command-activity__reason">{details.reason}</p>}
              <div className="run-command-shell" role="group" aria-label={t("agent.command.shell")}>
                <div className="run-command-shell__title">{t("agent.command.shell")}</div>
                {command && <pre className="run-command-shell__command">$ {command}</pre>}
                <pre className="run-command-shell__output">
                  {commandOutput || t("agent.command.noOutput")}
                </pre>
                <div
                  className="run-command-shell__status"
                  data-status={commandFailed ? "failed" : "succeeded"}
                >
                  <span aria-hidden="true">{commandFailed ? "×" : "✓"}</span>
                  <span>{getCommandStatus(commandResult, result?.ok, t)}</span>
                </div>
              </div>
            </>
          ) : (
            <>
              {details.reason && <p className="run-command-activity__reason">{details.reason}</p>}
              {command && <pre>{command}</pre>}
            </>
          )}
          {rejectedMessage && (
            <>
              <span>{t("agent.command.rejectReason")}</span>
              <p>{rejectedMessage}</p>
            </>
          )}
          {result?.error && !commandResult && (
            <>
              <span>{t("agent.detail.error")}</span>
              <pre>{result.error}</pre>
            </>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  );
}

export function RunCommandToolActivityGroup({ items }: RunCommandToolActivityGroupProps) {
  const { t } = useFrontendConfig();
  if (items.length === 0) return null;
  if (items.length === 1) {
    const item = items[0];
    return (
      <RunCommandToolActivity
        cancelled={item.cancelled}
        call={item.call}
        result={item.result}
      />
    );
  }

  const isPending = items.some((item) => getRunCommandStatus(item) === "running");

  return (
    <AgentActivityDisclosure
      className="agent-activity--run-command"
      hasDetails
      icon={SquareTerminal}
      isPending={isPending}
      label={getGroupLabel(items, t)}
    >
      <div className="agent-activity__details run-command-activity__details run-command-activity__details--group">
        {items.map((item) => (
          <RunCommandToolActivity
            cancelled={item.cancelled}
            call={item.call}
            key={item.call.id}
            result={item.result}
          />
        ))}
      </div>
    </AgentActivityDisclosure>
  );
}
