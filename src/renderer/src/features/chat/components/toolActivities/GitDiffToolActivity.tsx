// Renderer UI.
import { GitCompare } from "lucide-react";
import type { AgentToolCall, AgentToolResult } from "@mycopilot/protocol";
import type { TranslationKey } from "../../../../config/frontendTranslations";
import { useFrontendConfig } from "../../../../config/FrontendConfigProvider";
import { formatTranslation, type Translate } from "../../../../config/translationFormat";
import { AgentActivityDisclosure } from "./AgentActivityDisclosure";

interface GitDiffToolActivityProps {
  cancelled?: boolean;
  call: AgentToolCall;
  result?: AgentToolResult;
}

type GitDiffStatus = "running" | "completed" | "failed" | "cancelled";
type GitDiffFileStatus = "modified" | "added" | "deleted" | "renamed";

interface GitDiffFile {
  path: string;
  status: GitDiffFileStatus;
}

const STATUS_LABELS: Record<GitDiffStatus, TranslationKey> = {
  running: "agent.gitDiff.running",
  completed: "agent.gitDiff.completed",
  failed: "agent.gitDiff.failed",
  cancelled: "agent.gitDiff.cancelled",
};

const FILE_STATUS_LABELS: Record<GitDiffFileStatus, TranslationKey> = {
  modified: "agent.gitDiff.item.modified",
  added: "agent.gitDiff.item.added",
  deleted: "agent.gitDiff.item.deleted",
  renamed: "agent.gitDiff.item.renamed",
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function stringValue(value: unknown) {
  return typeof value === "string" ? value.trim() : "/protocol";
}

function booleanValue(value: unknown) {
  return typeof value === "boolean" ? value : false;
}

function getStatus(cancelled: boolean, result: AgentToolResult | undefined): GitDiffStatus {
  if (cancelled && !result) return "cancelled";
  if (result?.ok === false) return "failed";
  if (result) return "completed";
  return "running";
}

function getPatch(result: AgentToolResult | undefined) {
  if (!isRecord(result?.result)) return "/protocol";
  return stringValue(result.result.patch);
}

function getScope(call: AgentToolCall, result: AgentToolResult | undefined) {
  if (isRecord(result?.result)) {
    const resultPath = stringValue(result.result.path);
    if (resultPath) return resultPath;
  }

  if (!isRecord(call.args)) return "/protocol";
  return stringValue(call.args.path);
}

function isTruncated(result: AgentToolResult | undefined) {
  if (!isRecord(result?.result)) return false;
  return booleanValue(result.result.truncated);
}

function stripGitPrefix(path: string) {
  if (path === "/dev/null") return "/protocol";
  return path.replace(/^a\//, "/protocol").replace(/^b\//, "/protocol").trim();
}

function parseDiffHeader(line: string) {
  const match = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
  if (!match) return null;
  return {
    oldPath: match[1],
    newPath: match[2],
  };
}

function parseChangedFiles(patch: string): GitDiffFile[] {
  if (!patch.trim()) return [];

  const blocks: string[][] = [];
  let currentBlock: string[] = [];

  patch.split(/\r?\n/).forEach((line) => {
    if (line.startsWith("diff --git ")) {
      if (currentBlock.length > 0) {
        blocks.push(currentBlock);
      }
      currentBlock = [line];
      return;
    }

    if (currentBlock.length > 0) {
      currentBlock.push(line);
    }
  });

  if (currentBlock.length > 0) {
    blocks.push(currentBlock);
  }

  return blocks.reduce<GitDiffFile[]>((files, block) => {
    const header = parseDiffHeader(block[0] ?? "/protocol");
    if (!header) return files;

    const renameTo = block
      .find((line) => line.startsWith("rename to "))
      ?.replace(/^rename to /, "/protocol")
      .trim();
    const plusPath = block
      .find((line) => line.startsWith("+++ "))
      ?.replace(/^\+\+\+ /, "/protocol")
      .trim();
    const minusPath = block
      .find((line) => line.startsWith("--- "))
      ?.replace(/^--- /, "/protocol")
      .trim();

    let status: GitDiffFileStatus = "modified";
    let path = stripGitPrefix(plusPath || header.newPath);

    if (block.some((line) => line.startsWith("new file mode")) || minusPath === "/dev/null") {
      status = "added";
      path = stripGitPrefix(plusPath || header.newPath);
    } else if (block.some((line) => line.startsWith("deleted file mode")) || plusPath === "/dev/null") {
      status = "deleted";
      path = stripGitPrefix(minusPath || header.oldPath);
    } else if (renameTo) {
      status = "renamed";
      path = renameTo;
    }

    if (!path) return files;
    return [...files, { path, status }];
  }, []);
}

function GitDiffFileRow({ file }: { file: GitDiffFile }) {
  const { t } = useFrontendConfig();

  return (
    <div className="git-diff-activity__item" title={file.path}>
      {formatTranslation(t, FILE_STATUS_LABELS[file.status], { path: file.path })}
    </div>
  );
}

function getCompletedLabel(t: Translate, files: GitDiffFile[]) {
  if (files.length === 0) return t("agent.gitDiff.completed");
  return formatTranslation(t, "agent.gitDiff.completedCount", { count: files.length });
}

export function GitDiffToolActivity({
  cancelled = false,
  call,
  result,
}: GitDiffToolActivityProps) {
  const { t } = useFrontendConfig();
  const status = getStatus(cancelled, result);
  const patch = getPatch(result);
  const files = parseChangedFiles(patch);
  const scope = getScope(call, result);
  const hasDetails = status !== "running";
  const label = status === "completed" ? getCompletedLabel(t, files) : t(STATUS_LABELS[status]);

  return (
    <AgentActivityDisclosure
      className="agent-activity--git-diff"
      hasDetails={hasDetails}
      icon={GitCompare}
      isPending={status === "running"}
      label={label}
    >
      {hasDetails && (
        <div className="agent-activity__details git-diff-activity__details">
          {scope ? (
            <p className="git-diff-activity__scope">
              <span>{t("agent.gitDiff.scope")}</span>
              {scope}
            </p>
          ) : null}
          {result?.error ? <p className="git-diff-activity__error">{result.error}</p> : null}
          {status === "cancelled" && !result ? <p>{t("agent.gitDiff.cancelledDetail")}</p> : null}
          {!result?.error && status === "completed" && files.length === 0 && !patch.trim() ? (
            <p>{t("agent.gitDiff.empty")}</p>
          ) : null}
          {!result?.error && status === "completed" && files.length === 0 && patch.trim() ? (
            <p>{t("agent.gitDiff.summary")}</p>
          ) : null}
          {!result?.error && files.length > 0 ? (
            <div className="git-diff-activity__items">
              {files.map((file, index) => (
                <GitDiffFileRow file={file} key={`${file.status}:${file.path}:${index}`} />
              ))}
            </div>
          ) : null}
          {!result?.error && isTruncated(result) ? (
            <p className="git-diff-activity__truncated">{t("agent.gitDiff.truncated")}</p>
          ) : null}
        </div>
      )}
    </AgentActivityDisclosure>
  );
}
