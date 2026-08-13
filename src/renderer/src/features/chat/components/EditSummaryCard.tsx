import { ChevronDown, ChevronUp, FileDiff, Undo2 } from 'lucide-react'
import { useState, type JSX } from 'react'
import type { GitTurnDiffSummary } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'

interface EditSummaryCardProps {
  onReview?: (filePath?: string) => void
  readOnly?: boolean
  summary: GitTurnDiffSummary
}

interface EditSummaryEntry {
  additions?: number
  deletions?: number
  filePath: string
  id: string
}

const COLLAPSED_FILE_COUNT = 3

function splitFilePath(filePath: string): { directory: string; fileName: string } {
  const normalized = filePath.replace(/\\/g, '/')
  const separatorIndex = normalized.lastIndexOf('/')
  if (separatorIndex < 0) {
    return { directory: '', fileName: filePath }
  }

  return {
    directory: `${normalized.slice(0, separatorIndex + 1)}`,
    fileName: normalized.slice(separatorIndex + 1)
  }
}

function EditSummaryPath({
  entry,
  onReview
}: {
  entry: EditSummaryEntry
  onReview?: (filePath?: string) => void
}): JSX.Element {
  const { t } = useFrontendConfig()
  const { directory, fileName } = splitFilePath(entry.filePath)
  const content = (
    <>
      {directory && <span className="edit-summary-card__file-directory">{directory}</span>}
      <span className="edit-summary-card__file-name">{fileName || entry.filePath}</span>
    </>
  )

  if (!onReview) {
    return (
      <span className="edit-summary-card__file-path" title={entry.filePath}>
        {content}
      </span>
    )
  }

  return (
    <button
      aria-label={formatTranslation(t, 'agent.editSummary.reviewFile', {
        filePath: entry.filePath
      })}
      className="edit-summary-card__file-path edit-summary-card__file-button"
      onClick={() => onReview(entry.filePath)}
      title={formatTranslation(t, 'agent.editSummary.reviewFile', {
        filePath: entry.filePath
      })}
      type="button"
    >
      {content}
    </button>
  )
}

export function EditSummaryCard({
  onReview,
  readOnly = false,
  summary
}: EditSummaryCardProps): JSX.Element | null {
  const { t } = useFrontendConfig()
  const [expanded, setExpanded] = useState(false)
  const entries = summary.files.map((file) => ({
    additions: file.stats?.additions,
    deletions: file.stats?.deletions,
    filePath: file.path,
    id: file.path
  }))

  if (entries.length === 0) return null

  const visibleEntries = expanded ? entries : entries.slice(0, COLLAPSED_FILE_COUNT)
  const hiddenCount = entries.length - visibleEntries.length
  const totals = summary.stats

  return (
    <section className="edit-summary-card" aria-label={t('agent.editSummary.title')}>
      <div className="edit-summary-card__header">
        <span className="edit-summary-card__icon" aria-hidden="true">
          <FileDiff />
        </span>
        <div className="edit-summary-card__overview">
          <p>
            {formatTranslation(t, 'agent.editSummary.editedFiles', {
              count: String(entries.length)
            })}
          </p>
          <div className="edit-summary-card__totals" aria-label={t('agent.editSummary.lineStats')}>
            <span className="edit-summary-card__additions">+{totals.additions}</span>
            <span className="edit-summary-card__deletions">-{totals.deletions}</span>
          </div>
        </div>
        {!readOnly && (
          <div className="edit-summary-card__actions" aria-label={t('agent.editSummary.actions')}>
            <button className="edit-summary-card__undo" type="button">
              <span>{t('agent.editSummary.undo')}</span>
              <Undo2 aria-hidden="true" />
            </button>
            <button
              className="edit-summary-card__review"
              disabled={!onReview}
              onClick={() => onReview?.()}
              type="button"
            >
              {t('agent.editSummary.review')}
            </button>
          </div>
        )}
      </div>
      <div className="edit-summary-card__files">
        {visibleEntries.map((entry) => (
          <div className="edit-summary-card__file-row" key={entry.id}>
            <EditSummaryPath entry={entry} onReview={readOnly ? undefined : onReview} />
            {entry.additions !== undefined && entry.deletions !== undefined && (
              <span
                className="edit-summary-card__file-stats"
                aria-label={t('agent.editSummary.lineStats')}
              >
                <span className="edit-summary-card__additions">+{entry.additions}</span>
                <span className="edit-summary-card__deletions">-{entry.deletions}</span>
              </span>
            )}
          </div>
        ))}
      </div>
      {(hiddenCount > 0 || expanded) && (
        <button
          className="edit-summary-card__more"
          onClick={() => setExpanded((current) => !current)}
          type="button"
        >
          <span>
            {expanded
              ? t('agent.editSummary.showLess')
              : formatTranslation(t, 'agent.editSummary.showMore', { count: String(hiddenCount) })}
          </span>
          {expanded ? <ChevronUp aria-hidden="true" /> : <ChevronDown aria-hidden="true" />}
        </button>
      )}
    </section>
  )
}
