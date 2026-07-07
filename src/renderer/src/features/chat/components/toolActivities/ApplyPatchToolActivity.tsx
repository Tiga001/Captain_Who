// Renderer UI.
import { Pencil } from "lucide-react";
import type {
  AgentDiffProposal,
  AgentPatchOperation,
  AgentPatchResult,
  AgentToolCall,
  AgentToolResult,
} from "@mycopilot/protocol";
import type { TranslationKey } from "../../../../config/frontendTranslations";
import { useFrontendConfig } from "../../../../config/FrontendConfigProvider";
import { formatTranslation } from "../../../../config/translationFormat";
import { revealStoredProjectFile } from "../../../storage/storageClient";
import { AgentActivityDisclosure } from "./AgentActivityDisclosure";

interface ApplyPatchToolActivityProps {
  cancelled?: boolean;
  call: AgentToolCall;
  diff?: AgentDiffProposal;
  projectId?: string | null;
  result?: AgentToolResult;
}

export interface ApplyPatchToolActivityGroupItem extends ApplyPatchToolActivityProps {}

interface ApplyPatchToolActivityGroupProps {
  items: ApplyPatchToolActivityGroupItem[];
  projectId?: string | null;
}

export type ApplyPatchStatus = "waiting" | "running" | "applied" | "failed" | "rejected" | "cancelled";

const ROW_LABELS: Record<AgentPatchOperation, Record<ApplyPatchStatus, TranslationKey>> = {
  create: {
    waiting: "agent.patch.create.row.waiting",
    running: "agent.patch.create.row.running",
    applied: "agent.patch.create.row.applied",
    failed: "agent.patch.create.row.failed",
    rejected: "agent.patch.create.row.rejected",
    cancelled: "agent.patch.create.row.cancelled",
  },
  update: {
    waiting: "agent.patch.update.row.waiting",
    running: "agent.patch.update.row.running",
    applied: "agent.patch.update.row.applied",
    failed: "agent.patch.update.row.failed",
    rejected: "agent.patch.update.row.rejected",
    cancelled: "agent.patch.update.row.cancelled",
  },
  delete: {
    waiting: "agent.patch.delete.row.waiting",
    running: "agent.patch.delete.row.running",
    applied: "agent.patch.delete.row.applied",
    failed: "agent.patch.delete.row.failed",
    rejected: "agent.patch.delete.row.rejected",
    cancelled: "agent.patch.delete.row.cancelled",
  },
};

export function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

export function getString(value: unknown) {
  return typeof value === "string" ? value.trim() : "/protocol";
}

function getRawString(value: unknown) {
  return typeof value === "string" ? value : "/protocol";
}

function getNumber(value: unknown) {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, value) : undefined;
}

function getOperation(value: unknown): AgentPatchOperation | undefined {
  return value === "create" || value === "update" || value === "delete" ? value : undefined;
}

export function isAbsoluteLocalPath(filePath: string) {
  return filePath.startsWith("/") || /^[A-Za-z]:[\\/]/.test(filePath) || filePath.startsWith("\\\\");
}

export function getPatchResult(result: AgentToolResult | undefined): AgentPatchResult | undefined {
  if (!isRecord(result?.result)) return undefined;
  const status = result.result.status;
  const operation = getOperation(result.result.operation);
  const filePath = getString(result.result.filePath);
  const appliedFilePaths = result.result.appliedFilePaths;
  if (
    (status !== "applied" && status !== "failed" && status !== "rejected")
    || !operation
    || !filePath
    || !Array.isArray(appliedFilePaths)
  ) {
    return undefined;
  }

  return result.result as unknown as AgentPatchResult;
}

function getCallArgs(call: AgentToolCall) {
  return isRecord(call.args) ? call.args : {};
}

function getStatus(item: ApplyPatchToolActivityGroupItem): ApplyPatchStatus {
  const patchResult = getPatchResult(item.result);
  if (patchResult?.status === "applied") return "applied";
  if (patchResult?.status === "failed") return "failed";
  if (patchResult?.status === "rejected") return "rejected";
  if (item.result?.ok === false) return "failed";
  if (item.call.approvalStatus === "rejected") return "rejected";
  if (item.cancelled) return "cancelled";
  if (item.call.approvalStatus === "required") return "waiting";
  return "running";
}

export function countPatchLines(patch: string) {
  return patch.split(/\r?\n/).reduce(
    (counts, line) => {
      if (line.startsWith("+++ ") || line.startsWith("--- ")) return counts;
      if (line.startsWith("+")) counts.additions += 1;
      if (line.startsWith("-")) counts.deletions += 1;
      return counts;
    },
    { additions: 0, deletions: 0 },
  );
}

function countTextLines(content: string) {
  const normalized = content.replace(/\r\n/g, "\n").replace(/\n$/, "/protocol");
  if (!normalized) return 0;
  return normalized.split("\n").length;
}

function countStructuredEditLines(edits: unknown) {
  if (!Array.isArray(edits)) return undefined;

  return edits.reduce(
    (counts, edit) => {
      if (!isRecord(edit)) return counts;
      const kind = getString(edit.kind);
      if (kind === "replace") {
        counts.additions += countTextLines(getRawString(edit.newText));
        counts.deletions += countTextLines(getRawString(edit.oldText));
        return counts;
      }

      if (
        kind === "insert_before" ||
        kind === "insert_after" ||
        kind === "append" ||
        kind === "prepend"
      ) {
        counts.additions += countTextLines(getRawString(edit.text));
      }

      return counts;
    },
    { additions: 0, deletions: 0 },
  );
}

function getGitDiffPatch(value: unknown) {
  if (!isRecord(value)) return "/protocol";
  return getRawString(value.patch);
}

function getFallbackLineCounts(args: Record<string, unknown>, operation: AgentPatchOperation) {
  const structuredCounts = countStructuredEditLines(args.edits);
  if (structuredCounts) return structuredCounts;

  const content = getRawString(args.content);
  if (content && operation === "create") {
    return { additions: countTextLines(content), deletions: 0 };
  }

  return { additions: 0, deletions: 0 };
}

export function getApplyPatchItemView(item: ApplyPatchToolActivityGroupItem) {
  const args = getCallArgs(item.call);
  const patchResult = getPatchResult(item.result);
  const resultValue = isRecord(item.result?.result) ? item.result.result : {};
  const operation = patchResult?.operation ?? item.diff?.operation ?? getOperation(args.operation) ?? "update";
  const filePath = patchResult?.filePath ?? item.diff?.filePath ?? getString(args.filePath);
  const patch = item.diff?.patch || getRawString(args.patch) || getGitDiffPatch(resultValue.gitDiff);
  const parsedCounts = patch ? countPatchLines(patch) : getFallbackLineCounts(args, operation);
  const additions = getNumber(resultValue.additions) ?? getNumber(args.additions) ?? parsedCounts.additions;
  const deletions = getNumber(resultValue.deletions) ?? getNumber(args.deletions) ?? parsedCounts.deletions;

  return {
    additions,
    deletions,
    error: patchResult?.error ?? item.result?.error ?? "/protocol",
    filePath,
    message: patchResult?.message ?? "/protocol",
    operation,
    status: getStatus(item),
  };
}

function getGroupLabel(
  items: ApplyPatchToolActivityGroupItem[],
  t: ReturnType<typeof useFrontendConfig>["t"],
) {
  const counts = items.reduce(
    (currentCounts, item) => {
      currentCounts[getStatus(item)] += 1;
      return currentCounts;
    },
    { applied: 0, cancelled: 0, failed: 0, rejected: 0, running: 0, waiting: 0 },
  );
  const count = String(items.length);

  if (counts.waiting > 0) {
    return formatTranslation(t, "agent.patch.group.waiting", { count });
  }
  if (counts.running > 0) {
    return formatTranslation(t, "agent.patch.group.running", { count });
  }
  if (counts.failed === 0 && counts.rejected === 0 && counts.cancelled === 0) {
    return formatTranslation(t, "agent.patch.group.applied", { count });
  }

  const parts = [
    counts.applied > 0
      ? formatTranslation(t, "agent.patch.group.appliedCount", { count: String(counts.applied) })
      : "/protocol",
    counts.failed > 0
      ? formatTranslation(t, "agent.patch.group.failedCount", { count: String(counts.failed) })
      : "/protocol",
    counts.rejected > 0
      ? formatTranslation(t, "agent.patch.group.rejectedCount", { count: String(counts.rejected) })
      : "/protocol",
    counts.cancelled > 0
      ? formatTranslation(t, "agent.patch.group.cancelledCount", { count: String(counts.cancelled) })
      : "/protocol",
  ].filter(Boolean);

  return [formatTranslation(t, "agent.patch.group.processed", { count }), ...parts].join(
    t("agent.separator"),
  );
}

function ApplyPatchFileRow({
  item,
  projectId,
}: {
  item: ApplyPatchToolActivityGroupItem;
  projectId?: string | null;
}) {
  const { t } = useFrontendConfig();
  const view = getApplyPatchItemView(item);
  const note = view.message || view.error;
  const canReveal = Boolean(view.filePath && (projectId || isAbsoluteLocalPath(view.filePath)));

  return (
    <div className="apply-patch-activity__item">
      <div className="apply-patch-activity__item-line" title={view.filePath}>
        <span>{t(ROW_LABELS[view.operation][view.status])}</span>
        {canReveal ? (
          <button
            aria-label={formatTranslation(t, "agent.patch.revealFile", {
              filePath: view.filePath,
            })}
            className="apply-patch-activity__path"
            onClick={() => {
              void revealStoredProjectFile(projectId, view.filePath).catch((error) => {
                console.error("Failed to reveal patched file", error);
              });
            }}
            title={formatTranslation(t, "agent.patch.revealFile", {
              filePath: view.filePath,
            })}
            type="button"
          >
            {view.filePath}
          </button>
        ) : (
          <span className="apply-patch-activity__path">{view.filePath}</span>
        )}
        <span className="apply-patch-activity__additions">+{view.additions}</span>
        <span className="apply-patch-activity__deletions">-{view.deletions}</span>
      </div>
      {note ? <p className="apply-patch-activity__note">{note}</p> : null}
    </div>
  );
}

export function ApplyPatchToolActivity(props: ApplyPatchToolActivityProps) {
  return <ApplyPatchToolActivityGroup items={[props]} projectId={props.projectId} />;
}

export function ApplyPatchToolActivityGroup({
  items,
  projectId,
}: ApplyPatchToolActivityGroupProps) {
  const { t } = useFrontendConfig();
  if (items.length === 0) return null;
  const isPending = items.some((item) => {
    const status = getStatus(item);
    return status === "waiting" || status === "running";
  });

  return (
    <AgentActivityDisclosure
      className="agent-activity--apply-patch"
      hasDetails
      icon={Pencil}
      isPending={isPending}
      label={getGroupLabel(items, t)}
    >
      <div className="agent-activity__details apply-patch-activity__details">
        {items.map((item) => (
          <ApplyPatchFileRow item={item} key={item.call.id} projectId={projectId} />
        ))}
      </div>
    </AgentActivityDisclosure>
  );
}
