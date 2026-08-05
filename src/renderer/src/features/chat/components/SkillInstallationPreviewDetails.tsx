import type { AgentSkillInstallationPreview } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'

export function skillInstallationPlainText(value: unknown, fallback: string): string {
  if (typeof value !== 'string') return fallback
  const clean = Array.from(value, (character) => {
    const codePoint = character.codePointAt(0) ?? 0
    return codePoint <= 31 || codePoint === 127 ? ' ' : character
  })
    .join('')
    .trim()
  return clean.slice(0, 1_024) || fallback
}

function sourceLabel(source: unknown, fallback: string): string {
  if (!source || typeof source !== 'object') return fallback
  const record = source as Record<string, unknown>
  const kind = skillInstallationPlainText(record.kind, '')
  if (kind === 'github' || kind === 'githubRepository') {
    return skillInstallationPlainText(
      record.url,
      skillInstallationPlainText(record.repository, fallback)
    )
  }
  if (kind === 'localDirectory') {
    return skillInstallationPlainText(record.path, fallback)
  }
  return fallback
}

function resourceCount(summary: unknown, key: 'assets' | 'references' | 'scripts'): number {
  if (!summary || typeof summary !== 'object' || Array.isArray(summary)) return 0
  const value = (summary as Record<string, unknown>)[key]
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : 0
}

export function SkillInstallationSummaryDetails({
  description,
  resourceSummary,
  sourceSummary
}: {
  description: unknown
  resourceSummary: unknown
  sourceSummary: unknown
}) {
  const { t } = useFrontendConfig()
  const source = sourceLabel(sourceSummary, t('agent.skillInstallation.unknownSource'))

  return (
    <dl className="agent-skill-installation-approval__details">
      <div>
        <dt>{t('agent.skillInstallation.description')}</dt>
        <dd>
          {skillInstallationPlainText(description, t('agent.skillInstallation.noDescription'))}
        </dd>
      </div>
      <div>
        <dt>{t('agent.skillInstallation.source')}</dt>
        <dd>{source}</dd>
      </div>
      <div>
        <dt>{t('agent.skillInstallation.resources')}</dt>
        <dd>
          {formatTranslation(t, 'agent.skillInstallation.resourcesValue', {
            references: resourceCount(resourceSummary, 'references'),
            assets: resourceCount(resourceSummary, 'assets'),
            scripts: resourceCount(resourceSummary, 'scripts')
          })}
        </dd>
      </div>
    </dl>
  )
}

export function SkillInstallationPreviewDetails({
  preview
}: {
  preview: AgentSkillInstallationPreview
}) {
  return (
    <SkillInstallationSummaryDetails
      description={preview.description}
      resourceSummary={preview.resourceSummary}
      sourceSummary={preview.sourceSummary}
    />
  )
}
