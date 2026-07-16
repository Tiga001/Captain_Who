import type { ReactNode } from 'react'
import { ChevronDown, ChevronUp } from 'lucide-react'
import { formatTranslation } from '../../config/translationFormat'
import type { Translate } from '../../config/translationFormat'
import {
  DEFAULT_GIT_DIFF_EXPANSION_STEP,
  type GitDiffExpansionDirection,
  type GitDiffGapPosition
} from './diff'

export type GitReviewGapFragment = 'whole' | 'left' | 'right'

interface GitReviewGapSeparatorProps {
  fragment?: GitReviewGapFragment
  lineCount: number
  onExpand?: (direction: GitDiffExpansionDirection) => void
  position: GitDiffGapPosition
  t: Translate
}

/** A semantic omitted-range surface. Split right fragments are visual extensions only. */
export function GitReviewGapSeparator({
  fragment = 'whole',
  lineCount,
  onExpand,
  position,
  t
}: GitReviewGapSeparatorProps): ReactNode {
  if (fragment === 'right') {
    return (
      <div
        className="git-review__gap-separator git-review__gap-separator--fragment"
        data-fragment="right"
        data-position={position}
        aria-hidden="true"
      />
    )
  }

  const step = Math.min(DEFAULT_GIT_DIFF_EXPANSION_STEP, lineCount)
  const canExpandDown = Boolean(onExpand) && position !== 'leading'
  const canExpandUp = Boolean(onExpand) && position !== 'trailing'
  const lineLabel = formatTranslation(t, 'gitReview.diff.unmodifiedLines', { count: lineCount })
  const labelDirection: GitDiffExpansionDirection =
    position === 'leading' ? 'up' : position === 'trailing' ? 'down' : 'both'
  const labelAction =
    labelDirection === 'up'
      ? formatTranslation(t, 'gitReview.diff.expandUp', { count: step })
      : labelDirection === 'down'
        ? formatTranslation(t, 'gitReview.diff.expandDown', { count: step })
        : t('gitReview.diff.expandBoth')
  const className =
    fragment === 'left'
      ? 'git-review__gap-separator git-review__gap-separator--fragment'
      : 'git-review__gap-separator'

  return (
    <div
      className={className}
      data-fragment={fragment === 'left' ? 'left' : undefined}
      data-position={position}
    >
      <span className="git-review__gap-gutter" aria-hidden="true">
        {canExpandDown && (
          <button
            type="button"
            aria-label={formatTranslation(t, 'gitReview.diff.expandDown', { count: step })}
            onClick={() => onExpand?.('down')}
          >
            <ChevronDown aria-hidden="true" />
          </button>
        )}
        {canExpandUp && (
          <button
            type="button"
            aria-label={formatTranslation(t, 'gitReview.diff.expandUp', { count: step })}
            onClick={() => onExpand?.('up')}
          >
            <ChevronUp aria-hidden="true" />
          </button>
        )}
      </span>
      {onExpand ? (
        <button
          className="git-review__gap-label"
          type="button"
          aria-label={`${lineLabel}. ${labelAction}`}
          onClick={() => onExpand(labelDirection)}
        >
          {lineLabel}
        </button>
      ) : (
        <span className="git-review__gap-label">{lineLabel}</span>
      )}
    </div>
  )
}
