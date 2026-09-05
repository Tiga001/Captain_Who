import type { ReactNode, RefObject } from 'react'
import type { GitReviewCommit } from '@mycopilot/protocol'
import { Check, LoaderCircle, RefreshCw } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { GitReviewMenuPortal } from './GitReviewMenuPortal'

export type GitReviewCommitListState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { error: string; status: 'error' }
  | { commits: GitReviewCommit[]; status: 'ready'; truncated: boolean }

interface GitReviewCommitSubmenuProps {
  anchorRef: RefObject<HTMLElement | null>
  autoFocus: boolean
  language: string
  onCloseAll: () => void
  onCloseSubmenu: () => void
  onRetry: () => void
  onSelect: (commit: GitReviewCommit) => void
  selectedSha?: string
  state: GitReviewCommitListState
  t: Translate
}

export function GitReviewCommitSubmenu({
  anchorRef,
  autoFocus,
  language,
  onCloseAll,
  onCloseSubmenu,
  onRetry,
  onSelect,
  selectedSha,
  state,
  t
}: GitReviewCommitSubmenuProps): ReactNode {
  return (
    <GitReviewMenuPortal
      anchorRef={anchorRef}
      ariaLabel={t('gitReview.commit.menu')}
      autoFocus={
        autoFocus && state.status !== 'idle' && state.status !== 'loading' ? 'selected' : 'none'
      }
      className="git-review__commit-menu"
      onArrowLeft={onCloseSubmenu}
      onEscape={onCloseAll}
      placement="side-start"
    >
      {state.status === 'idle' || state.status === 'loading' ? (
        <div className="git-review__menu-state" role="status">
          <LoaderCircle className="git-review__spinner" aria-hidden="true" />
          <span>{t('gitReview.commit.loading')}</span>
        </div>
      ) : state.status === 'error' ? (
        <div className="git-review__menu-state git-review__menu-state--error" role="alert">
          <span>{t('gitReview.commit.error')}</span>
          <button type="button" role="menuitem" onClick={onRetry}>
            <RefreshCw aria-hidden="true" />
            {t('gitReview.retry')}
          </button>
        </div>
      ) : state.commits.length === 0 ? (
        <div className="git-review__menu-state" role="status">
          {t('gitReview.commit.empty')}
        </div>
      ) : (
        <div className="git-review__commit-list">
          {state.commits.map((commit) => {
            const checked = commit.sha === selectedSha
            return (
              <button
                aria-checked={checked}
                className="git-review__commit-item"
                key={commit.sha}
                role="menuitemradio"
                title={commitTooltipText(commit, language)}
                type="button"
                onClick={() => onSelect(commit)}
              >
                <span className="git-review__commit-copy">
                  <strong>{commit.subject || commit.sha.slice(0, 12)}</strong>
                  <span>
                    <time dateTime={commit.committedAt}>
                      {formatCommitRelativeTime(commit.committedAt, language)}
                    </time>
                    <span className="git-review__commit-stats" aria-hidden="true">
                      <span>+{commit.stats.lineCountsComplete ? commit.stats.additions : '?'}</span>
                      <span>-{commit.stats.lineCountsComplete ? commit.stats.deletions : '?'}</span>
                    </span>
                  </span>
                </span>
                {checked && <Check className="git-review__menu-check" aria-hidden="true" />}
              </button>
            )
          })}
          {state.truncated && (
            <div className="git-review__menu-state git-review__menu-state--compact" role="status">
              {t('gitReview.commit.truncated')}
            </div>
          )}
        </div>
      )}
    </GitReviewMenuPortal>
  )
}

export function formatCommitRelativeTime(
  value: string,
  language: string,
  now = Date.now()
): string {
  const timestamp = Date.parse(value)
  if (!Number.isFinite(timestamp)) return value
  const delta = timestamp - now
  const absolute = Math.abs(delta)
  const formatter = new Intl.RelativeTimeFormat(language, { numeric: 'auto' })
  if (absolute < 60_000) return formatter.format(Math.round(delta / 1_000), 'second')
  if (absolute < 3_600_000) return formatter.format(Math.round(delta / 60_000), 'minute')
  if (absolute < 86_400_000) return formatter.format(Math.round(delta / 3_600_000), 'hour')
  if (absolute < 2_592_000_000) return formatter.format(Math.round(delta / 86_400_000), 'day')
  if (absolute < 31_536_000_000) return formatter.format(Math.round(delta / 2_592_000_000), 'month')
  return formatter.format(Math.round(delta / 31_536_000_000), 'year')
}

function commitTooltipText(commit: GitReviewCommit, language: string): string {
  const committedAt = formatAbsoluteCommitTime(commit.committedAt, language)
  const stats = `+${commit.stats.additions} -${commit.stats.deletions}`
  return [commit.subject, commit.sha.slice(0, 12), committedAt, stats].filter(Boolean).join(' · ')
}

function formatAbsoluteCommitTime(value: string, language: string): string {
  const timestamp = Date.parse(value)
  if (!Number.isFinite(timestamp)) return value
  return new Intl.DateTimeFormat(language, {
    dateStyle: 'medium',
    timeStyle: 'short'
  }).format(timestamp)
}
