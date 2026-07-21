import type { SkillDescriptor, SkillSelection, SkillsListOutput } from '@mycopilot/protocol'
import { AlertTriangle, LoaderCircle, RefreshCw, Search, Sparkles, X } from 'lucide-react'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { SkillIcon } from '../../skills/SkillIcon'
import { getSkillPresentation } from '../../skills/skillPresentation'
import {
  filterSkillDescriptors,
  getSkillFallbackName,
  matchSkillSelection,
  MAX_SELECTED_SKILLS
} from '../../skills/skillSelection'
import type { SkillCatalogState } from '../../skills/useSkillCatalog'

interface ComposerSelectedSkillsProps {
  catalog?: SkillsListOutput
  onRemove: (skillId: string) => void
  selections: readonly SkillSelection[]
}

type SkillSourceKind = SkillDescriptor['source']['kind']
type SkillTrust = SkillDescriptor['trust']

interface SkillDisplayProvenance {
  sourceKind: SkillSourceKind
  trust: SkillTrust
}

function getSkillSourceLabel(
  sourceKind: SkillSourceKind,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  switch (sourceKind) {
    case 'workspace':
      return t('chat.workspaceSkill')
    case 'bundled':
      return t('chat.bundledSkill')
    case 'installed':
      return t('chat.installedSkill')
  }
}

function getSkillDisplayProvenance(
  skillId: string,
  descriptor?: SkillDescriptor
): SkillDisplayProvenance | undefined {
  if (descriptor) {
    return {
      sourceKind: descriptor.source.kind,
      trust: descriptor.trust
    }
  }

  // A selection intentionally stores only the opaque id and revision. The source prefix remains
  // sufficient to keep supported schema-v4 provenance visible while a catalog is loading.
  if (skillId.startsWith('workspace:')) {
    return { sourceKind: 'workspace', trust: 'untrusted' }
  }
  if (skillId.startsWith('bundled:')) {
    return { sourceKind: 'bundled', trust: 'application' }
  }
  if (skillId.startsWith('installed:')) {
    return { sourceKind: 'installed', trust: 'untrusted' }
  }
  return undefined
}

export function ComposerSelectedSkills({
  catalog,
  onRemove,
  selections
}: ComposerSelectedSkillsProps) {
  const { t } = useFrontendConfig()
  if (selections.length === 0) return null

  const descriptors = catalog ? filterSkillDescriptors(catalog.skills, '') : []

  return (
    <div className="chat-composer__skills" aria-label={t('chat.selectedSkills')}>
      {selections.map((selection) => {
        const catalogMatch = catalog ? matchSkillSelection(selection, descriptors) : undefined
        const match =
          catalog?.truncated && catalogMatch?.status === 'unavailable' ? undefined : catalogMatch
        const presentation = getSkillPresentation(
          {
            description: match?.descriptor?.description,
            id: selection.id,
            name: match?.descriptor?.name ?? getSkillFallbackName(selection.id),
            source: match?.descriptor?.source
          },
          t
        )
        const name = presentation.name
        const provenance = getSkillDisplayProvenance(selection.id, match?.descriptor)
        const sourceLabel = provenance ? getSkillSourceLabel(provenance.sourceKind, t) : undefined
        const trustLabel = provenance
          ? provenance.trust === 'untrusted'
            ? t('chat.skillTrustUntrusted')
            : t('chat.skillTrustApplication')
          : undefined
        const provenanceLabel =
          sourceLabel && trustLabel ? `${sourceLabel} · ${trustLabel}` : undefined
        const accessibleName = provenanceLabel ? `${name} · ${provenanceLabel}` : name

        return (
          <span
            className="composer-skill-chip"
            data-source-kind={provenance?.sourceKind}
            data-status={match?.status === 'current' ? undefined : match?.status}
            data-trust={provenance?.trust}
            key={`${selection.id}:${selection.revision}`}
            title={
              match?.status === 'stale'
                ? t('chat.skillStaleDescription')
                : match?.status === 'unavailable'
                  ? t('chat.skillUnavailableDescription')
                  : name
            }
          >
            <SkillIcon
              className="composer-skill-chip__icon"
              skillId={selection.id}
              source={match?.descriptor?.source}
            />
            <span className="composer-skill-chip__content">
              <span className="composer-skill-chip__name">{name}</span>
              {provenanceLabel && (
                <small className="composer-skill-chip__provenance">{provenanceLabel}</small>
              )}
            </span>
            {match?.status !== 'current' && match && <AlertTriangle aria-hidden="true" />}
            <button
              aria-label={`${t('chat.removeSkill')} ${accessibleName}`}
              onClick={() => onRemove(selection.id)}
              type="button"
            >
              <X aria-hidden="true" />
            </button>
          </span>
        )
      })}
    </div>
  )
}

interface ComposerSkillPickerProps {
  catalogState: SkillCatalogState
  onClose: () => void
  onRefresh: () => void
  onSearchChange: (value: string) => void
  onToggle: (descriptor: SkillDescriptor) => void
  onUseLatest: (descriptor: SkillDescriptor) => void
  projectId: string | null
  search: string
  selections: readonly SkillSelection[]
}

export function ComposerSkillPicker({
  catalogState,
  onClose,
  onRefresh,
  onSearchChange,
  onToggle,
  onUseLatest,
  projectId,
  search,
  selections
}: ComposerSkillPickerProps) {
  const { t } = useFrontendConfig()
  const readyOutput =
    catalogState.status === 'ready' && catalogState.projectId === projectId
      ? catalogState.output
      : undefined
  const presentedSkills = readyOutput
    ? readyOutput.skills.map((skill) => ({ ...skill, ...getSkillPresentation(skill, t) }))
    : []
  const visibleSkills = filterSkillDescriptors(presentedSkills, search)
  const allSkills = filterSkillDescriptors(presentedSkills, '')
  const staleSelections = readyOutput
    ? selections.filter((selection) => matchSkillSelection(selection, allSkills).status === 'stale')
    : []
  const unavailableSelections =
    readyOutput && !readyOutput.truncated
      ? selections.filter(
          (selection) => matchSkillSelection(selection, allSkills).status === 'unavailable'
        )
      : []

  return (
    <div
      className="composer-skill-menu"
      role="dialog"
      aria-label={t('chat.skills')}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          event.stopPropagation()
          onClose()
        }
      }}
    >
      <div className="composer-skill-menu__header">
        <div>
          <strong>{t('chat.skills')}</strong>
          <span>
            {t('chat.skillSelectionCount')
              .replace('{selected}', String(selections.length))
              .replace('{maximum}', String(MAX_SELECTED_SKILLS))}
          </span>
        </div>
        <button type="button" onClick={onClose} aria-label={t('chat.closeSkills')}>
          <X aria-hidden="true" />
        </button>
      </div>

      {!projectId ? (
        <div className="composer-skill-menu__state">
          <Sparkles aria-hidden="true" />
          <strong>{t('chat.skillProjectRequired')}</strong>
          <span>{t('chat.skillProjectRequiredDescription')}</span>
        </div>
      ) : (
        <>
          <label className="composer-skill-menu__search">
            <Search aria-hidden="true" />
            <input
              autoFocus
              value={search}
              placeholder={t('chat.searchSkills')}
              aria-label={t('chat.searchSkills')}
              onChange={(event) => onSearchChange(event.target.value)}
            />
          </label>

          {catalogState.status === 'loading' && (
            <div className="composer-skill-menu__state" role="status">
              <LoaderCircle className="composer-skill-menu__spinner" aria-hidden="true" />
              <span>{t('chat.loadingSkills')}</span>
            </div>
          )}

          {catalogState.status === 'error' && (
            <div className="composer-skill-menu__state" role="alert">
              <AlertTriangle aria-hidden="true" />
              <strong>{t('chat.skillsLoadFailed')}</strong>
              <span>{catalogState.message}</span>
              <button type="button" onClick={onRefresh}>
                <RefreshCw aria-hidden="true" />
                <span>{t('chat.retrySkills')}</span>
              </button>
            </div>
          )}

          {readyOutput && (
            <>
              {(staleSelections.length > 0 || unavailableSelections.length > 0) && (
                <div className="composer-skill-menu__notice" role="alert">
                  <AlertTriangle aria-hidden="true" />
                  <span>
                    {staleSelections.length > 0
                      ? t('chat.skillStaleDescription')
                      : t('chat.skillUnavailableDescription')}
                  </span>
                </div>
              )}

              {readyOutput.truncated && (
                <div className="composer-skill-menu__notice" role="status">
                  <AlertTriangle aria-hidden="true" />
                  <span>{t('chat.skillCatalogTruncated')}</span>
                </div>
              )}

              {readyOutput.diagnostics.length > 0 && (
                <details className="composer-skill-menu__diagnostics">
                  <summary>
                    {t('chat.skillDiagnostics').replace(
                      '{count}',
                      String(readyOutput.diagnostics.length)
                    )}
                  </summary>
                  <ul>
                    {readyOutput.diagnostics.slice(0, 5).map((diagnostic, index) => (
                      <li key={`${diagnostic.code}:${diagnostic.skillId ?? 'catalog'}:${index}`}>
                        {diagnostic.message}
                      </li>
                    ))}
                  </ul>
                </details>
              )}

              {allSkills.length === 0 ? (
                <div className="composer-skill-menu__state">
                  <Sparkles aria-hidden="true" />
                  <strong>{t('chat.noSkills')}</strong>
                  <span>{t('chat.noSkillsDescription')}</span>
                </div>
              ) : visibleSkills.length === 0 ? (
                <div className="composer-skill-menu__state">
                  <Search aria-hidden="true" />
                  <span>{t('chat.noMatchingSkills')}</span>
                </div>
              ) : (
                <div
                  className="composer-skill-menu__items"
                  role="list"
                  aria-label={t('chat.skills')}
                >
                  {visibleSkills.map((skill) => {
                    const selected = selections.find((selection) => selection.id === skill.id)
                    const isStale = Boolean(selected && selected.revision !== skill.revision)
                    const atLimit = !selected && selections.length >= MAX_SELECTED_SKILLS
                    const sourceLabel = getSkillSourceLabel(skill.source.kind, t)
                    const trustLabel =
                      skill.trust === 'untrusted'
                        ? t('chat.skillTrustUntrusted')
                        : t('chat.skillTrustApplication')
                    const provenanceLabel = `${sourceLabel} · ${trustLabel}`

                    return (
                      <div
                        className="composer-skill-option"
                        data-selected={Boolean(selected) || undefined}
                        data-source-kind={skill.source.kind}
                        data-stale={isStale || undefined}
                        data-trust={skill.trust}
                        key={skill.id}
                        role="listitem"
                      >
                        <Tooltip
                          anchorClassName="composer-skill-option__tooltip-anchor"
                          content={skill.description}
                          describeTrigger
                          preferredPlacement="top"
                        >
                          <button
                            className="composer-skill-option__main"
                            type="button"
                            aria-label={`${skill.name} · ${provenanceLabel}`}
                            aria-pressed={Boolean(selected)}
                            disabled={atLimit}
                            onClick={() => onToggle(skill)}
                          >
                            <SkillIcon
                              className="composer-skill-option__icon"
                              skillId={skill.id}
                              source={skill.source}
                            />
                            <strong className="composer-skill-option__name">{skill.name}</strong>
                            {isStale && <AlertTriangle aria-hidden="true" />}
                          </button>
                        </Tooltip>
                        {isStale && (
                          <button
                            className="composer-skill-option__update"
                            type="button"
                            onClick={() => onUseLatest(skill)}
                          >
                            {t('chat.useLatestSkill')}
                          </button>
                        )}
                      </div>
                    )
                  })}
                </div>
              )}

              {selections.length >= MAX_SELECTED_SKILLS && (
                <p className="composer-skill-menu__limit" role="status">
                  {t('chat.skillSelectionLimit').replace('{maximum}', String(MAX_SELECTED_SKILLS))}
                </p>
              )}
            </>
          )}
        </>
      )}
    </div>
  )
}
