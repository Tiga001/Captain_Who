import { useEffect, useId, useMemo } from 'react'
import type { ReactNode } from 'react'
import type {
  GitReviewFile,
  GitReviewFileMutationAction,
  GitReviewScope
} from '@mycopilot/protocol'
import {
  AlertCircle,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  LoaderCircle,
  Minus,
  Plus,
  RefreshCw,
  Undo2
} from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { GitReviewFileIcon } from './GitReviewFileIcon'
import { buildSplitDiffRows, parseGitPatch } from './gitPatchParser'
import type { GitDiffLine, GitSplitDiffRow } from './gitPatchParser'
import type { GitReviewDiffState } from './useGitReview'

export type GitReviewViewMode = 'unified' | 'split'

interface GitDiffCardProps {
  diffState?: GitReviewDiffState
  file: GitReviewFile
  isExpanded: boolean
  mutationLocked: boolean
  mutationPending: boolean
  onMutate: (fileId: string, action: GitReviewFileMutationAction) => void
  onRequestDiff: (fileId: string) => void
  onRestore: (file: GitReviewFile) => void
  onToggle: (fileId: string) => void
  scope: GitReviewScope
  t: Translate
  viewMode: GitReviewViewMode
  wrapLines: boolean
}

export function GitDiffCard({
  diffState,
  file,
  isExpanded,
  mutationLocked,
  mutationPending,
  onMutate,
  onRequestDiff,
  onRestore,
  onToggle,
  scope,
  t,
  viewMode,
  wrapLines
}: GitDiffCardProps): ReactNode {
  useEffect(() => {
    if (isExpanded) onRequestDiff(file.id)
  }, [file.id, isExpanded, onRequestDiff])

  const patch = diffState?.status === 'ready' ? diffState.value.patch : undefined
  const parsedPatch = useMemo(() => parseGitPatch(patch ?? ''), [patch])
  const pathTitle = file.previousPath ? `${file.previousPath} → ${file.path}` : file.path
  const actionLabel = isExpanded ? t('gitReview.file.collapse') : t('gitReview.file.expand')
  const statusLabel = t(`gitReview.status.${file.status}`)
  const statsLabel = file.stats ? `, +${file.stats.additions} -${file.stats.deletions}` : ''

  return (
    <section
      className="git-review__diff-card"
      data-expanded={isExpanded ? 'true' : undefined}
      data-file-id={file.id}
    >
      <div className="git-review__diff-card-header">
        <button
          className="git-review__diff-card-main"
          type="button"
          aria-expanded={isExpanded}
          aria-label={`${actionLabel}: ${pathTitle}, ${statusLabel}${statsLabel}`}
          title={pathTitle}
          onClick={() => onToggle(file.id)}
        >
          <GitReviewFileIcon path={file.path} />
          <GitReviewFilePath file={file} />
          {file.stats && <GitReviewFileStats file={file} />}
        </button>
        <div className="git-review__file-actions" aria-label={t('gitReview.file.actions')}>
          <FileActionButton label={actionLabel} onClick={() => onToggle(file.id)}>
            {isExpanded ? <ChevronDown aria-hidden="true" /> : <ChevronRight aria-hidden="true" />}
          </FileActionButton>
          <FileActionButton
            ariaDisabled
            label={t('gitReview.file.openSoon')}
            onClick={() => undefined}
          >
            <ExternalLink aria-hidden="true" />
          </FileActionButton>
          {scope === 'unstaged' && (
            <FileActionButton
              disabled={mutationLocked}
              label={t('gitReview.file.restore')}
              onClick={() => onRestore(file)}
            >
              <Undo2 aria-hidden="true" />
            </FileActionButton>
          )}
          <FileActionButton
            disabled={mutationLocked}
            label={scope === 'unstaged' ? t('gitReview.file.stage') : t('gitReview.file.unstage')}
            onClick={() => onMutate(file.id, scope === 'unstaged' ? 'stage' : 'unstage')}
          >
            {mutationPending ? (
              <LoaderCircle className="git-review__spinner" aria-hidden="true" />
            ) : scope === 'unstaged' ? (
              <Plus aria-hidden="true" />
            ) : (
              <Minus aria-hidden="true" />
            )}
          </FileActionButton>
        </div>
      </div>

      {isExpanded && (
        <div className="git-review__diff-card-body">
          <GitDiffStateContent
            diffState={diffState}
            fileId={file.id}
            hunks={parsedPatch.hunks}
            onRequestDiff={onRequestDiff}
            t={t}
            viewMode={viewMode}
            wrapLines={wrapLines}
          />
        </div>
      )}
    </section>
  )
}

function GitReviewFilePath({ file }: { file: GitReviewFile }): ReactNode {
  const separatorIndex = file.path.lastIndexOf('/')
  const directory = separatorIndex >= 0 ? file.path.slice(0, separatorIndex + 1) : ''
  const name = separatorIndex >= 0 ? file.path.slice(separatorIndex + 1) : file.path

  return (
    <span className="git-review__file-path">
      <bdi>
        <span className="git-review__file-directory">
          {file.previousPath && `${file.previousPath} → `}
          {directory}
        </span>
        <span className="git-review__file-name">{name}</span>
      </bdi>
    </span>
  )
}

interface FileActionButtonProps {
  ariaDisabled?: boolean
  children: ReactNode
  disabled?: boolean
  label: string
  onClick: () => void
}

function FileActionButton({
  ariaDisabled,
  children,
  disabled,
  label,
  onClick
}: FileActionButtonProps): ReactNode {
  const tooltipId = useId()
  return (
    <span className="git-review__file-action-wrap">
      <button
        className="git-review__file-action-button"
        type="button"
        aria-describedby={tooltipId}
        aria-disabled={ariaDisabled || undefined}
        aria-label={label}
        disabled={disabled}
        onClick={(event) => {
          event.stopPropagation()
          if (!ariaDisabled) onClick()
        }}
      >
        {children}
      </button>
      <span className="git-review__file-action-tooltip" id={tooltipId} role="tooltip">
        {label}
      </span>
    </span>
  )
}

function GitReviewFileStats({ file }: { file: GitReviewFile }): ReactNode {
  if (!file.stats) return null
  return (
    <span className="git-review__file-stats">
      <span className="git-review__file-additions">+{file.stats.additions}</span>
      <span className="git-review__file-deletions">-{file.stats.deletions}</span>
    </span>
  )
}

interface GitDiffStateContentProps {
  diffState?: GitReviewDiffState
  fileId: string
  hunks: ReturnType<typeof parseGitPatch>['hunks']
  onRequestDiff: (fileId: string) => void
  t: Translate
  viewMode: GitReviewViewMode
  wrapLines: boolean
}

function GitDiffStateContent({
  diffState,
  fileId,
  hunks,
  onRequestDiff,
  t,
  viewMode,
  wrapLines
}: GitDiffStateContentProps): ReactNode {
  if (!diffState || diffState.status === 'idle' || diffState.status === 'loading') {
    return (
      <div className="git-review__diff-message">
        <LoaderCircle className="git-review__spinner" aria-hidden="true" />
        <span>{t('gitReview.diff.loading')}</span>
      </div>
    )
  }

  if (diffState.status === 'error') {
    return (
      <div className="git-review__diff-message git-review__diff-message--error" role="alert">
        <AlertCircle aria-hidden="true" />
        <span>{diffState.error}</span>
        <button type="button" onClick={() => onRequestDiff(fileId)}>
          <RefreshCw aria-hidden="true" />
          {t('gitReview.retry')}
        </button>
      </div>
    )
  }

  if (diffState.value.status === 'binary') {
    return <div className="git-review__diff-message">{t('gitReview.diff.binary')}</div>
  }
  if (diffState.value.status === 'tooLarge') {
    return <div className="git-review__diff-message">{t('gitReview.diff.tooLarge')}</div>
  }
  if (hunks.length === 0) {
    return <div className="git-review__diff-message">{t('gitReview.diff.noHunks')}</div>
  }

  return viewMode === 'unified' ? (
    <UnifiedDiff hunks={hunks} wrapLines={wrapLines} />
  ) : (
    <SplitDiff hunks={hunks} wrapLines={wrapLines} />
  )
}

interface ParsedDiffProps {
  hunks: ReturnType<typeof parseGitPatch>['hunks']
  wrapLines: boolean
}

function UnifiedDiff({ hunks, wrapLines }: ParsedDiffProps): ReactNode {
  return (
    <div className="git-review__unified-diff" data-wrap={wrapLines ? 'true' : undefined}>
      {hunks.map((hunk, hunkIndex) => (
        <div className="git-review__hunk" key={`${hunk.header}-${hunkIndex}`}>
          <div className="git-review__hunk-header">{hunk.header}</div>
          {hunk.lines.map((line, lineIndex) => (
            <div
              className={`git-review__unified-line git-review__diff-line--${line.kind}`}
              key={`${hunkIndex}-${lineIndex}`}
            >
              <span className="git-review__line-number">{line.oldLineNumber ?? ''}</span>
              <span className="git-review__line-number">{line.newLineNumber ?? ''}</span>
              <DiffCode line={line} />
            </div>
          ))}
        </div>
      ))}
    </div>
  )
}

function SplitDiff({ hunks, wrapLines }: ParsedDiffProps): ReactNode {
  return (
    <div className="git-review__split-diff" data-wrap={wrapLines ? 'true' : undefined}>
      {hunks.map((hunk, hunkIndex) => (
        <div className="git-review__hunk" key={`${hunk.header}-${hunkIndex}`}>
          <div className="git-review__hunk-header">{hunk.header}</div>
          {buildSplitDiffRows(hunk).map((row, rowIndex) => (
            <SplitDiffRow key={`${hunkIndex}-${rowIndex}`} row={row} />
          ))}
        </div>
      ))}
    </div>
  )
}

function SplitDiffRow({ row }: { row: GitSplitDiffRow }): ReactNode {
  return (
    <div className="git-review__split-row">
      <SplitDiffSide line={row.left} side="left" />
      <SplitDiffSide line={row.right} side="right" />
    </div>
  )
}

function SplitDiffSide({ line, side }: { line?: GitDiffLine; side: 'left' | 'right' }): ReactNode {
  const lineNumber = side === 'left' ? line?.oldLineNumber : line?.newLineNumber
  return (
    <div className={`git-review__split-side${line ? ` git-review__diff-line--${line.kind}` : ''}`}>
      <span className="git-review__line-number">{lineNumber ?? ''}</span>
      {line ? <DiffCode line={line} /> : <span className="git-review__diff-code" />}
    </div>
  )
}

function DiffCode({ line }: { line: GitDiffLine }): ReactNode {
  const prefix =
    line.kind === 'addition'
      ? '+'
      : line.kind === 'deletion'
        ? '-'
        : line.kind === 'meta'
          ? ''
          : ' '
  return (
    <span className="git-review__diff-code">
      <span className="git-review__diff-prefix" aria-hidden="true">
        {prefix}
      </span>
      {line.content || ' '}
    </span>
  )
}
