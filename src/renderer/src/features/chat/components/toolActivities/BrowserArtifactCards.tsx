import type { BrowserArtifactReference } from '@mycopilot/protocol'
import {
  Braces,
  Download,
  FileArchive,
  FileText,
  Image as ImageIcon,
  LoaderCircle,
  Video,
  XCircle
} from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { hostClient } from '../../../../host/hostClient'

type PreviewState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'image'; src: string }
  | { status: 'text'; text: string; truncated: boolean }
  | { status: 'failed' }

type ExportState = 'idle' | 'exporting' | 'exported' | 'failed'

const MAX_RENDERED_TEXT_CHARACTERS = 4_096

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes >= 10 * 1024 ? 0 : 1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(bytes >= 10 * 1024 * 1024 ? 0 : 1)} MB`
}

function ArtifactIcon({ artifact }: { artifact: BrowserArtifactReference }) {
  switch (artifact.kind) {
    case 'image':
      return <ImageIcon aria-hidden="true" />
    case 'trace':
      return <FileArchive aria-hidden="true" />
    case 'video':
      return <Video aria-hidden="true" />
    case 'download':
      return <Download aria-hidden="true" />
    case 'json':
      return <Braces aria-hidden="true" />
    default:
      return <FileText aria-hidden="true" />
  }
}

function BrowserArtifactCard({ artifact }: { artifact: BrowserArtifactReference }) {
  const { t } = useFrontendConfig()
  const exportInFlight = useRef(false)
  const [exportState, setExportState] = useState<ExportState>('idle')
  const [preview, setPreview] = useState<PreviewState>({ status: 'idle' })

  useEffect(
    () => () => {
      if (preview.status === 'image') URL.revokeObjectURL(preview.src)
    },
    [preview]
  )

  const loadPreview = async (): Promise<void> => {
    if (artifact.preview === 'none' || preview.status === 'loading') return
    if (preview.status === 'image') URL.revokeObjectURL(preview.src)
    setPreview({ status: 'loading' })
    const result = await hostClient.browser.readArtifactPreview({ schemaVersion: 1, artifact })
    if (!result.ok) {
      setPreview({ status: 'failed' })
      return
    }
    const bytes = Uint8Array.from(result.value.bytes)
    if (artifact.preview === 'image') {
      const src = URL.createObjectURL(new Blob([bytes], { type: artifact.mimeType }))
      setPreview({ status: 'image', src })
      return
    }
    try {
      const fullText = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
      setPreview({
        status: 'text',
        text: fullText.slice(0, MAX_RENDERED_TEXT_CHARACTERS),
        truncated: fullText.length > MAX_RENDERED_TEXT_CHARACTERS
      })
    } catch {
      setPreview({ status: 'failed' })
    }
  }

  const exportArtifact = async (): Promise<void> => {
    if (exportInFlight.current) return
    exportInFlight.current = true
    setExportState('exporting')
    try {
      const result = await hostClient.browser.exportArtifact({ schemaVersion: 1, artifact })
      if (!result.ok) {
        setExportState('failed')
        return
      }
      setExportState(result.value.status === 'exported' ? 'exported' : 'idle')
    } catch {
      setExportState('failed')
    } finally {
      exportInFlight.current = false
    }
  }

  return (
    <article className="browser-artifact-card" data-kind={artifact.kind}>
      <div className="browser-artifact-card__summary">
        <span className="browser-artifact-card__icon">
          <ArtifactIcon artifact={artifact} />
        </span>
        <span className="browser-artifact-card__content">
          <span className="browser-artifact-card__name" title={artifact.displayName}>
            {artifact.displayName}
          </span>
          <span>
            {artifact.mimeType} · {formatBytes(artifact.sizeBytes)}
          </span>
        </span>
        <span className="browser-artifact-card__actions">
          {artifact.preview !== 'none' ? (
            <button
              disabled={preview.status === 'loading'}
              onClick={() => void loadPreview()}
              type="button"
            >
              {preview.status === 'loading' ? (
                <LoaderCircle aria-hidden="true" />
              ) : (
                t('agent.builtinCapability.artifact.preview')
              )}
            </button>
          ) : null}
          <button
            disabled={exportState === 'exporting'}
            onClick={() => void exportArtifact()}
            type="button"
          >
            {exportState === 'exporting' ? (
              <LoaderCircle aria-hidden="true" />
            ) : exportState === 'exported' ? (
              t('agent.builtinCapability.artifact.exported')
            ) : (
              t('agent.builtinCapability.artifact.export')
            )}
          </button>
        </span>
      </div>
      {preview.status === 'image' ? (
        <div className="browser-artifact-card__image-preview">
          <img alt={artifact.displayName} draggable={false} src={preview.src} />
        </div>
      ) : null}
      {preview.status === 'text' ? (
        <pre className="browser-artifact-card__text-preview">
          {preview.text}
          {preview.truncated ? `\n${t('agent.builtinCapability.artifact.previewTruncated')}` : ''}
        </pre>
      ) : null}
      {preview.status === 'failed' ? (
        <span className="browser-artifact-card__preview-failed">
          <XCircle aria-hidden="true" />
          {t('agent.builtinCapability.artifact.previewUnavailable')}
        </span>
      ) : null}
      {exportState === 'failed' ? (
        <span className="browser-artifact-card__export-failed">
          <XCircle aria-hidden="true" />
          {t('agent.builtinCapability.artifact.exportUnavailable')}
        </span>
      ) : null}
    </article>
  )
}

export function BrowserArtifactCards({
  artifacts
}: {
  artifacts: readonly BrowserArtifactReference[]
}) {
  const { t } = useFrontendConfig()
  if (artifacts.length === 0) return null
  return (
    <section
      aria-label={t('agent.builtinCapability.artifact.list')}
      className="browser-artifact-list"
    >
      {artifacts.map((artifact) => (
        <BrowserArtifactCard artifact={artifact} key={artifact.artifactId} />
      ))}
    </section>
  )
}
