import { useEffect, useRef, useState, type JSX } from 'react'
import { ChevronDown, ChevronRight, FilePenLine, LoaderCircle } from 'lucide-react'
import type { AgentFileDraftSnapshot } from '@mycopilot/protocol'
import { getAgentFileWriteDiff, readAgentFileDraft } from '../../../agent/agentClient'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { Translate } from '../../../../config/translationFormat'

interface FileWriteToolActivityProps {
  draft?: AgentFileDraftSnapshot
  draftId: string
}

function AnimatedInteger({ value }: { value: number }): JSX.Element {
  const [displayed, setDisplayed] = useState(value)
  const previousRef = useRef(value)

  useEffect(() => {
    const from = previousRef.current
    previousRef.current = value
    if (from === value || window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      setDisplayed(value)
      return
    }
    const startedAt = performance.now()
    const duration = 260
    let frameId = 0
    const update = (now: number): void => {
      const progress = Math.min(1, (now - startedAt) / duration)
      const eased = 1 - Math.pow(1 - progress, 3)
      setDisplayed(Math.round(from + (value - from) * eased))
      if (progress < 1) frameId = window.requestAnimationFrame(update)
    }
    frameId = window.requestAnimationFrame(update)
    return () => window.cancelAnimationFrame(frameId)
  }, [value])

  return <>{displayed}</>
}

function statusLabel(draft: AgentFileDraftSnapshot | undefined, t: Translate): string {
  if (!draft) return t('agent.fileWrite.preparing')
  if (draft.status === 'waiting_approval') return t('agent.fileWrite.waiting')
  if (draft.status === 'applying') return t('agent.fileWrite.applying')
  if (draft.status === 'applied') return t('agent.fileWrite.applied')
  if (draft.status === 'failed' || draft.status === 'conflict') return t('agent.fileWrite.failed')
  if (draft.status === 'aborted' || draft.status === 'expired') return t('agent.fileWrite.stopped')
  return draft.mode === 'modify' || draft.mode === 'append'
    ? t('agent.fileWrite.editing')
    : t('agent.fileWrite.generating')
}

function isBusy(draft: AgentFileDraftSnapshot | undefined): boolean {
  return !draft || ['drafting', 'ready', 'applying'].includes(draft.status)
}

export function FileWriteToolActivity({ draft, draftId }: FileWriteToolActivityProps): JSX.Element {
  const { t } = useFrontendConfig()
  const [expanded, setExpanded] = useState(false)
  const [preview, setPreview] = useState('')
  const [previewError, setPreviewError] = useState('')
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!expanded) return
    let cancelled = false
    const request =
      draft?.status === 'waiting_approval' || draft?.status === 'applied'
        ? getAgentFileWriteDiff(draftId).then((page) => page.patch)
        : readAgentFileDraft(draftId).then((page) => page.content)
    void request
      .then((content) => {
        if (!cancelled) setPreview(content)
      })
      .catch((error) => {
        if (!cancelled) setPreviewError(error instanceof Error ? error.message : String(error))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [draft?.status, draft?.updatedAt, draftId, expanded])

  const toggleExpanded = (): void => {
    if (!expanded) {
      setLoading(true)
      setPreviewError('')
    }
    setExpanded((value) => !value)
  }

  const filePath = draft?.filePath ?? t('agent.fileWrite.unknownFile')
  return (
    <div className="agent-activity agent-activity--file-write">
      <div className="file-write-activity__summary">
        <span className="file-write-activity__icon" aria-hidden="true">
          {isBusy(draft) ? (
            <LoaderCircle className="file-write-activity__spinner" />
          ) : (
            <FilePenLine />
          )}
        </span>
        <span className="file-write-activity__label">{statusLabel(draft, t)}</span>
        <span className="file-write-activity__path" title={filePath}>
          {filePath}
        </span>
        <span
          className="file-write-activity__stats"
          aria-label={`+${draft?.additions ?? 0} -${draft?.deletions ?? 0}`}
        >
          <span className="file-write-activity__additions">
            +<AnimatedInteger value={draft?.additions ?? 0} />
          </span>
          <span className="file-write-activity__deletions">
            -<AnimatedInteger value={draft?.deletions ?? 0} />
          </span>
        </span>
        <button
          aria-label={t('agent.fileWrite.togglePreview')}
          className="file-write-activity__toggle"
          onClick={toggleExpanded}
          title={t('agent.fileWrite.togglePreview')}
          type="button"
        >
          {expanded ? <ChevronDown /> : <ChevronRight />}
        </button>
      </div>
      {expanded ? (
        <div className="file-write-activity__preview">
          {loading ? <span>{t('agent.fileWrite.loadingPreview')}</span> : null}
          {previewError ? <span className="file-write-activity__error">{previewError}</span> : null}
          {!loading && !previewError ? <pre>{preview}</pre> : null}
        </div>
      ) : null}
    </div>
  )
}
