// Renderer summary cards for successfully created or modified Office artifacts in one agent run.

import { useState } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { OfficeFileIcon } from '../../../components/files/OfficeFileIcon'
import type { ChatAgentRunView } from '../chatTypes'
import {
  getOfficeArtifactEntries,
  getOfficeArtifactFileName,
  hasUnresolvedPathAlias,
  isAbsoluteLocalPath,
  type OfficeArtifactEntry
} from '../skillOfficeActivity'
import { revealStoredProjectFile } from '../../storage/storageClient'
import { hostClient } from '../../../host/hostClient'

async function downloadManagedArtifact(
  entry: OfficeArtifactEntry,
  conversationId: string,
  observerRootConversationId?: string
): Promise<void> {
  if (!entry.managedArtifact) throw new Error('Managed document identity is unavailable.')
  const result = await hostClient.imageGeneration.readArtifact({
    schemaVersion: 1,
    artifact: entry.managedArtifact,
    conversationId,
    ...(observerRootConversationId ? { observerRootConversationId } : {})
  })
  if (!result.ok) throw new Error(result.error.message)
  const bytes = Uint8Array.from(result.value.bytes)
  const objectUrl = URL.createObjectURL(new Blob([bytes], { type: entry.managedArtifact.mimeType }))
  const link = document.createElement('a')
  link.href = objectUrl
  link.download = getOfficeArtifactFileName(entry.displayName ?? entry.path)
  link.rel = 'noopener'
  document.body.appendChild(link)
  link.click()
  link.remove()
  window.setTimeout(() => URL.revokeObjectURL(objectUrl), 0)
}

function formatManagedArtifactSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function artifactTypeLabel(
  entry: OfficeArtifactEntry,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  const extension = (entry.displayName ?? entry.path).split('.').at(-1)?.toUpperCase() ?? ''
  const kind =
    entry.artifactKind === 'spreadsheet'
      ? t('agent.office.kind.spreadsheet')
      : entry.artifactKind === 'presentation'
        ? t('agent.office.kind.presentation')
        : t('agent.office.kind.document')
  return extension ? `${kind} · ${extension}` : kind
}

function ArtifactIcon({ kind }: { kind: OfficeArtifactEntry['artifactKind'] }) {
  return <OfficeFileIcon kind={kind} />
}

function OfficeArtifactCard({
  assistantMessageId,
  entry,
  projectId,
  conversationId,
  observerRootConversationId
}: {
  assistantMessageId?: string
  entry: OfficeArtifactEntry
  projectId?: string | null
  conversationId?: string
  observerRootConversationId?: string
}) {
  const { t } = useFrontendConfig()
  const [revealFailed, setRevealFailed] = useState(false)
  const isManagedArtifact = Boolean(entry.managedReadPath)
  const canReveal =
    !isManagedArtifact &&
    !hasUnresolvedPathAlias(entry.path) &&
    (entry.scope === 'workspace' ? Boolean(projectId) : isAbsoluteLocalPath(entry.path))

  return (
    <article
      className="office-artifact-card"
      data-kind={entry.artifactKind}
      aria-label={getOfficeArtifactFileName(entry.path)}
    >
      <span className="office-artifact-card__icon">
        <ArtifactIcon kind={entry.artifactKind} />
      </span>
      <div className="office-artifact-card__content">
        <p title={isManagedArtifact ? entry.displayName : entry.path}>
          {entry.displayName ?? getOfficeArtifactFileName(entry.path)}
        </p>
        <span>
          {isManagedArtifact
            ? `${artifactTypeLabel(entry, t)}${
                entry.managedArtifact
                  ? ` · ${formatManagedArtifactSize(entry.managedArtifact.sizeBytes)}`
                  : ''
              }`
            : artifactTypeLabel(entry, t)}
        </span>
      </div>
      <button
        disabled={isManagedArtifact ? !conversationId || !entry.managedArtifact : !canReveal}
        onClick={() => {
          if (entry.managedReadPath) {
            if (!conversationId) return
            void downloadManagedArtifact(entry, conversationId, observerRootConversationId).catch(
              (error) => {
                console.error('Failed to download managed document Artifact', error)
              }
            )
            return
          }
          if (!canReveal) return
          setRevealFailed(false)
          void revealStoredProjectFile(projectId, entry.path, assistantMessageId).catch((error) => {
            setRevealFailed(true)
            console.error('Failed to reveal Office artifact', error)
          })
        }}
        title={
          isManagedArtifact
            ? t('imagePreview.download')
            : canReveal
              ? t('agent.office.reveal')
              : t('agent.office.revealUnavailable')
        }
        type="button"
      >
        {isManagedArtifact ? t('imagePreview.download') : t('agent.office.reveal')}
      </button>
      {revealFailed ? <span role="status">{t('agent.office.revealUnavailable')}</span> : null}
    </article>
  )
}

export function OfficeArtifactsCard({
  assistantMessageId,
  conversationId,
  observerRootConversationId,
  projectId,
  run
}: {
  assistantMessageId?: string
  conversationId?: string
  observerRootConversationId?: string
  projectId?: string | null
  run: ChatAgentRunView
}) {
  const { t } = useFrontendConfig()
  const entries = getOfficeArtifactEntries(run)
  if (entries.length === 0) return null

  return (
    <section className="office-artifact-list" aria-label={t('agent.office.files')}>
      {entries.map((entry) => (
        <OfficeArtifactCard
          assistantMessageId={assistantMessageId}
          conversationId={conversationId}
          observerRootConversationId={observerRootConversationId}
          entry={entry}
          key={entry.id}
          projectId={projectId}
        />
      ))}
    </section>
  )
}
