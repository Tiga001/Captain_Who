import type { ReactNode } from 'react'
import type {
  GitReviewBranch,
  GitReviewCommit,
  GitReviewContext,
  GitReviewTarget
} from '@mycopilot/protocol'
import { ArrowRight, GitBranch, GitCommitHorizontal } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { copyTextToClipboard } from '../../components/clipboard'
import { Tooltip } from '../../components/overlay/Tooltip'
import { GitReviewBranchPicker, middleEllipsis } from './GitReviewBranchPicker'
import { GitReviewCopyButton } from './GitReviewCopyButton'
import type { GitReviewRepositoryContextState } from './useGitReviewRepositoryContext'

interface GitReviewContextRowProps {
  commitPreview?: GitReviewCommit
  isActive: boolean
  language: string
  onRetryBranches: () => void
  onSelectBranch: (branch: GitReviewBranch) => void
  repositoryState: GitReviewRepositoryContextState
  summaryContext?: GitReviewContext
  t: Translate
  target: GitReviewTarget
}

export function GitReviewContextRow({
  commitPreview,
  isActive,
  language,
  onRetryBranches,
  onSelectBranch,
  repositoryState,
  summaryContext,
  t,
  target
}: GitReviewContextRowProps): ReactNode {
  if (target.kind === 'commit') {
    const commit =
      summaryContext?.commit?.sha === target.commitSha
        ? summaryContext.commit
        : commitPreview?.sha === target.commitSha
          ? commitPreview
          : undefined
    const commitInfo = commit ? formatCommitInfo(commit, language) : undefined
    const title = commitInfo?.[0] || target.commitSha.slice(0, 12)
    return (
      <div className="git-review__context-row" data-kind="commit">
        <GitCommitHorizontal aria-hidden="true" />
        <Tooltip
          anchorClassName="git-review__context-commit-anchor"
          content={commitInfo ? <CommitTooltip lines={commitInfo} /> : target.commitSha}
          preferredPlacement="bottom"
        >
          <span className="git-review__context-commit-title">{title}</span>
        </Tooltip>
        <GitReviewCopyButton
          key={target.commitSha}
          anchorClassName="git-review__context-copy"
          disabled={!commitInfo}
          label={t('gitReview.commit.copyInfo')}
          onCopy={async () => {
            if (commitInfo) await copyTextToClipboard(commitInfo.join('\n'))
          }}
          t={t}
        />
      </div>
    )
  }

  if (target.kind !== 'branch') return null
  const currentBranch =
    summaryContext?.currentBranch ??
    (repositoryState.status === 'ready' ? repositoryState.value.currentBranch : undefined) ??
    t('gitReview.branch.detached')

  return (
    <div className="git-review__context-row" data-kind="branch">
      <GitBranch aria-hidden="true" />
      <Tooltip
        anchorClassName="git-review__context-current-anchor"
        content={currentBranch}
        preferredPlacement="bottom"
      >
        <span className="git-review__context-current-branch">{middleEllipsis(currentBranch)}</span>
      </Tooltip>
      <ArrowRight className="git-review__context-arrow" aria-hidden="true" />
      <GitReviewBranchPicker
        isActive={isActive}
        onRetry={onRetryBranches}
        onSelect={onSelectBranch}
        selectedRef={target.baseRef}
        state={repositoryState}
        t={t}
      />
    </div>
  )
}

function formatCommitInfo(
  commit: GitReviewCommit,
  language: string
): [string, string, string, string] {
  const timestamp = Date.parse(commit.committedAt)
  const date = Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(language, { dateStyle: 'medium', timeStyle: 'short' }).format(
        timestamp
      )
    : commit.committedAt
  return [
    commit.subject || commit.sha.slice(0, 12),
    commit.sha.slice(0, 12),
    date,
    `+${commit.stats.additions} -${commit.stats.deletions}`
  ]
}

function CommitTooltip({ lines }: { lines: [string, string, string, string] }) {
  return (
    <span className="git-review__commit-tooltip">
      <strong>{lines[0]}</strong>
      <span>{lines[1]}</span>
      <span>{lines[2]}</span>
      <span>{lines[3]}</span>
    </span>
  )
}
