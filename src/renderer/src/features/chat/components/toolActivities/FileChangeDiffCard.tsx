import type { GitReviewFile, GitReviewFileStatus } from '@mycopilot/protocol'
import { Check, Copy } from 'lucide-react'
import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { GitPatchRenderer } from '../../../gitReview/GitReviewDiffRenderer'
import '../../../gitReview/GitReviewPanel.css'
import { copyTextToClipboard } from '../clipboard'

const COPIED_INDICATOR_DURATION_MS = 1_600

interface FileChangeDiffCardProps {
  additions: number
  complete: boolean
  deletions: number
  error: string
  filePath: string
  hasMore: boolean
  loading: boolean
  onLoadMore: () => void
  patch: string
  toolCallId: string
}

const HUNK_HEADER_PATTERN = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/

function getVisualStatus(additions: number, deletions: number): GitReviewFileStatus {
  if (additions > 0 && deletions === 0) return 'added'
  if (deletions > 0 && additions === 0) return 'deleted'
  return 'modified'
}

/** Keeps a streamed/page-truncated final hunk renderable without inventing source context. */
export function getRenderableFileChangePatch(patch: string, complete: boolean): string {
  if (complete || !patch) return patch

  const normalized = patch.replaceAll('\r\n', '\n')
  const rawLines = normalized.split('\n')
  if (!normalized.endsWith('\n')) rawLines.pop()

  const output: string[] = []
  let index = 0
  while (index < rawLines.length) {
    const line = rawLines[index] ?? ''
    const header = line.match(HUNK_HEADER_PATTERN)
    if (!header) {
      output.push(line)
      index += 1
      continue
    }

    const hunkLines: string[] = []
    index += 1
    while (index < rawLines.length && !(rawLines[index] ?? '').startsWith('@@')) {
      const hunkLine = rawLines[index] ?? ''
      if (/^[ +\\-]/.test(hunkLine)) hunkLines.push(hunkLine)
      index += 1
    }

    let oldCount = 0
    let newCount = 0
    for (const hunkLine of hunkLines) {
      if (hunkLine.startsWith(' ') || hunkLine.startsWith('-')) oldCount += 1
      if (hunkLine.startsWith(' ') || hunkLine.startsWith('+')) newCount += 1
    }
    if (oldCount === 0 && newCount === 0) continue

    output.push(
      `@@ -${header[1]},${oldCount} +${header[3]},${newCount} @@${header[5] ?? ''}`,
      ...hunkLines
    )
  }

  return output.join('\n')
}

function FileChangeDiffCardComponent({
  additions,
  complete,
  deletions,
  error,
  filePath,
  hasMore,
  loading,
  onLoadMore,
  patch,
  toolCallId
}: FileChangeDiffCardProps): ReactNode {
  const { t } = useFrontendConfig()
  const [copied, setCopied] = useState(false)
  const copiedTimeoutRef = useRef<number | null>(null)
  const renderablePatch = useMemo(
    () => getRenderableFileChangePatch(patch, complete),
    [complete, patch]
  )
  const file = useMemo<GitReviewFile>(
    () => ({
      id: toolCallId,
      path: filePath,
      stats: { additions, deletions },
      status: getVisualStatus(additions, deletions)
    }),
    [additions, deletions, filePath, toolCallId]
  )

  useEffect(
    () => () => {
      if (copiedTimeoutRef.current !== null) window.clearTimeout(copiedTimeoutRef.current)
    },
    []
  )

  const handleCopy = async (): Promise<void> => {
    try {
      await copyTextToClipboard(patch)
      setCopied(true)
      if (copiedTimeoutRef.current !== null) window.clearTimeout(copiedTimeoutRef.current)
      copiedTimeoutRef.current = window.setTimeout(() => {
        setCopied(false)
        copiedTimeoutRef.current = null
      }, COPIED_INDICATOR_DURATION_MS)
    } catch (copyError) {
      console.error('Failed to copy Apply Patch Diff', copyError)
    }
  }

  const copyLabel = copied ? t('chat.copied') : t('chat.copy')

  return (
    <section className="file-change-diff-card" data-complete={complete ? 'true' : undefined}>
      <header className="file-change-diff-card__header">
        <span className="file-change-diff-card__path" title={filePath}>
          {filePath}
        </span>
        <span className="file-change-diff-card__stats" aria-label={`+${additions} -${deletions}`}>
          <span className="file-change-diff-card__additions">+{additions}</span>
          <span className="file-change-diff-card__deletions">-{deletions}</span>
        </span>
        <button
          aria-label={copyLabel}
          className={`file-change-diff-card__copy${copied ? ' is-copied' : ''}`}
          disabled={!patch}
          onClick={() => void handleCopy()}
          title={copyLabel}
          type="button"
        >
          {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
        </button>
      </header>
      <div className="file-change-diff-card__body">
        {error ? (
          <div className="file-change-diff-card__message file-change-diff-card__message--error">
            {error}
          </div>
        ) : renderablePatch ? (
          <div className="file-change-diff-card__renderer git-review">
            <GitPatchRenderer
              emptyState={t('gitReview.diff.noHunks')}
              file={file}
              invalidState={
                complete ? t('gitReview.diff.invalid') : t('agent.fileChange.loadingPreview')
              }
              patch={renderablePatch}
              snapshotId={toolCallId}
              syntaxHighlightingEnabled
              t={t}
              tooLargeState={t('gitReview.diff.tooLarge')}
              viewMode="split"
              wrapLines={false}
            />
          </div>
        ) : (
          <div className="file-change-diff-card__message">
            {loading || !complete
              ? t('agent.fileChange.loadingPreview')
              : t('gitReview.diff.noHunks')}
          </div>
        )}
        {hasMore && !error ? (
          <button
            className="file-change-diff-card__load-more"
            disabled={loading}
            onClick={onLoadMore}
            type="button"
          >
            {loading ? t('agent.fileChange.loadingPreview') : t('agent.fileChange.loadMorePreview')}
          </button>
        ) : null}
      </div>
    </section>
  )
}

export const FileChangeDiffCard = memo(FileChangeDiffCardComponent)

FileChangeDiffCard.displayName = 'FileChangeDiffCard'
