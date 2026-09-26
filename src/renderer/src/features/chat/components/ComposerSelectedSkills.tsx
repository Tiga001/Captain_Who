import type { SkillDescriptor, SkillSelection, SkillsListOutput } from '@mycopilot/protocol'
import { AlertTriangle, X } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { SkillIcon } from '../../skills/SkillIcon'
import { getSkillPresentation } from '../../skills/skillPresentation'
import {
  filterSkillDescriptors,
  getSkillFallbackName,
  matchSkillSelection
} from '../../skills/skillSelection'

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
        const hiddenProvenanceLabel =
          provenance?.sourceKind === 'bundled' && provenance.trust === 'application'
            ? undefined
            : provenanceLabel
        const statusLabel =
          match?.status === 'stale'
            ? t('chat.skillStaleDescription')
            : match?.status === 'unavailable'
              ? t('chat.skillUnavailableDescription')
              : undefined

        return (
          <span
            className="composer-skill-chip"
            data-source-kind={provenance?.sourceKind}
            data-status={match?.status === 'current' ? undefined : match?.status}
            data-trust={provenance?.trust}
            key={`${selection.id}:${selection.revision}`}
            title={[name, provenanceLabel, statusLabel].filter(Boolean).join(' · ')}
          >
            <span className="composer-skill-chip__link">
              <SkillIcon
                className="composer-skill-chip__icon"
                skillId={selection.id}
                source={match?.descriptor?.source}
              />
              <span className="composer-skill-chip__name">{name}</span>
              {hiddenProvenanceLabel && (
                <span className="composer-skill-chip__provenance">{hiddenProvenanceLabel}</span>
              )}
            </span>
            {match?.status !== 'current' && match && <AlertTriangle aria-hidden="true" />}
            <button
              className="composer-skill-chip__remove"
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
