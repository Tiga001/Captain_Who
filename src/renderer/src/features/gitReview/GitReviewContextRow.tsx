import type { ReactNode } from 'react'
import type {
  GitReviewBranch,
  GitReviewCommit,
  GitReviewContext,
  GitReviewTarget
} from '@mycopilot/protocol'
import { ArrowRight, GitBranch, GitCommitHorizontal } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { Tooltip } from '../../components/overlay/Tooltip'
import { GitReviewBranchPicker, middleEllipsis } from './GitReviewBranchPicker'
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
    const title = commit?.subject || target.commitSha.slice(0, 12)
    return (
      <div className="git-review__context-row" data-kind="commit">
        <GitCommitHorizontal aria-hidden="true" />
        <Tooltip
          anchorClassName="git-review__context-commit-anchor"
          content={
            commit ? <CommitTooltip commit={commit} language={language} /> : target.commitSha
          }
          preferredPlacement="bottom"
        >
          <span className="git-review__context-commit-title">{title}</span>
        </Tooltip>
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

function CommitTooltip({ commit, language }: { commit: GitReviewCommit; language: string }) {
  const timestamp = Date.parse(commit.committedAt)
  const date = Number.isFinite(timestamp)
    ? new Intl.DateTimeFormat(language, { dateStyle: 'medium', timeStyle: 'short' }).format(
        timestamp
      )
    : commit.committedAt
  return (
    <span className="git-review__commit-tooltip">
      <strong>{commit.subject || commit.sha.slice(0, 12)}</strong>
      <span>{commit.sha.slice(0, 12)}</span>
      <span>{date}</span>
      <span>
        +{commit.stats.additions} -{commit.stats.deletions}
      </span>
    </span>
  )
}
