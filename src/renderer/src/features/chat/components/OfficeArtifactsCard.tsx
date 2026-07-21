// Renderer summary cards for successfully created or modified Office artifacts in one agent run.

import { FileSpreadsheet, FileText, Presentation } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatAgentRunView } from '../chatTypes'
import {
  getOfficeArtifactEntries,
  getOfficeArtifactFileName,
  hasUnresolvedPathAlias,
  isAbsoluteLocalPath,
  type OfficeArtifactEntry
} from '../skillOfficeActivity'
import { revealStoredProjectFile } from '../../storage/storageClient'

function artifactTypeLabel(
  entry: OfficeArtifactEntry,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  const extension = entry.path.split('.').at(-1)?.toUpperCase() ?? ''
  const kind =
    entry.artifactKind === 'spreadsheet'
      ? t('agent.office.kind.spreadsheet')
      : entry.artifactKind === 'presentation'
        ? t('agent.office.kind.presentation')
        : t('agent.office.kind.document')
  return extension ? `${kind} · ${extension}` : kind
}

function ArtifactIcon({ kind }: { kind: OfficeArtifactEntry['artifactKind'] }) {
  if (kind === 'spreadsheet') return <FileSpreadsheet aria-hidden="true" />
  if (kind === 'presentation') return <Presentation aria-hidden="true" />
  return <FileText aria-hidden="true" />
}

function OfficeArtifactCard({
  entry,
  projectId
}: {
  entry: OfficeArtifactEntry
  projectId?: string | null
}) {
  const { t } = useFrontendConfig()
  const canReveal =
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
        <p title={entry.path}>{getOfficeArtifactFileName(entry.path)}</p>
        <span>{artifactTypeLabel(entry, t)}</span>
      </div>
      <button
        disabled={!canReveal}
        onClick={() => {
          if (!canReveal) return
          void revealStoredProjectFile(
            entry.scope === 'workspace' ? projectId : undefined,
            entry.path
          ).catch((error) => {
            console.error('Failed to reveal Office artifact', error)
          })
        }}
        title={canReveal ? t('agent.office.reveal') : t('agent.office.revealUnavailable')}
        type="button"
      >
        {t('agent.office.reveal')}
      </button>
    </article>
  )
}

export function OfficeArtifactsCard({
  projectId,
  run
}: {
  projectId?: string | null
  run: ChatAgentRunView
}) {
  const { t } = useFrontendConfig()
  const entries = getOfficeArtifactEntries(run)
  if (entries.length === 0) return null

  return (
    <section className="office-artifact-list" aria-label={t('agent.office.files')}>
      {entries.map((entry) => (
        <OfficeArtifactCard entry={entry} key={entry.id} projectId={projectId} />
      ))}
    </section>
  )
}
