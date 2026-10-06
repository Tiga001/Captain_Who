import {
  Brain,
  ChevronDown,
  FilePlus2,
  FileText,
  FileX2,
  Files,
  FolderOpen,
  Pencil,
  Search,
  SquareTerminal
} from 'lucide-react'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import type { ChatAgentRunView } from '../../chatTypes'
import type { BasicToolItem } from '../basicToolTimeline'
import {
  getBasicToolGroupPresentation,
  type BasicToolItemStatus,
  type BasicToolSummaryCategory
} from './basicToolStatus'
import { RollingLineCount } from './RollingLineCount'

const CATEGORY_ICONS = {
  read: FileText,
  search: Search,
  workspace: FolderOpen,
  attachments: Files,
  history: Brain,
  edit: Pencil,
  create: FilePlus2,
  delete: FileX2,
  command: SquareTerminal
}
const SUMMARY_ICON_ORDER: BasicToolSummaryCategory[] = [
  'edit',
  'create',
  'delete',
  'command',
  'search',
  'read',
  'workspace',
  'attachments',
  'history'
]
const CATEGORY_ORDER: BasicToolSummaryCategory[] = [
  'read',
  'search',
  'workspace',
  'attachments',
  'history',
  'edit',
  'create',
  'delete',
  'command'
]
const SUMMARY_KEYS: Record<BasicToolSummaryCategory, TranslationKey> = {
  read: 'agent.basicTools.summary.read',
  search: 'agent.basicTools.summary.search',
  workspace: 'agent.basicTools.summary.workspace',
  attachments: 'agent.basicTools.summary.attachments',
  history: 'agent.basicTools.summary.history',
  edit: 'agent.basicTools.summary.edit',
  create: 'agent.basicTools.summary.create',
  delete: 'agent.basicTools.summary.delete',
  command: 'agent.basicTools.summary.command'
}
const COMPLETED_KEYS: Record<BasicToolSummaryCategory, TranslationKey> = {
  read: 'agent.basicTools.completed.read',
  search: 'agent.basicTools.completed.search',
  workspace: 'agent.basicTools.completed.workspace',
  attachments: 'agent.basicTools.completed.attachments',
  history: 'agent.basicTools.completed.history',
  edit: 'agent.basicTools.completed.edit',
  create: 'agent.basicTools.completed.create',
  delete: 'agent.basicTools.completed.delete',
  command: 'agent.basicTools.completed.command'
}

function activeLabel(status: BasicToolItemStatus, t: Translate): string {
  if (status.phase === 'awaiting_approval') return t('agent.basicTools.waitingApproval')
  if (status.phase === 'ready') return t('agent.basicTools.edit.ready')
  if (status.phase === 'starting') return t('agent.command.starting')
  switch (status.item.category) {
    case 'read':
      return t('agent.activity.read.running')
    case 'search':
      return t('agent.activity.search.running')
    case 'workspace':
      return t(
        status.subject ? 'agent.activity.read.running' : 'agent.basicTools.workspace.running'
      )
    case 'attachments':
      return t(
        status.call.tool === 'attachments_list_project'
          ? 'agent.attachments.project.running'
          : 'agent.attachments.running'
      )
    case 'history':
      return t('agent.history.running')
    case 'edit':
      return t(
        status.editOperation === 'create'
          ? 'agent.basicTools.edit.creating'
          : status.editOperation === 'delete'
            ? 'agent.basicTools.edit.deleting'
            : 'agent.basicTools.edit.editing'
      )
    case 'command':
      return t('agent.command.running')
  }
}

export function BasicToolActivityHeader({
  run,
  items,
  expanded,
  onToggle,
  controls
}: {
  run: ChatAgentRunView
  items: readonly BasicToolItem[]
  expanded: boolean
  onToggle: () => void
  controls: string
}) {
  const { t } = useFrontendConfig()
  const presentation = getBasicToolGroupPresentation(run, items)
  const selected = presentation.selected
  const selectedAction = selected ? activeLabel(selected, t) : undefined
  let subject = selected?.subject
  if (selected && ['read', 'edit', 'workspace'].includes(selected.item.category)) {
    subject = selected.subject.split(/[\\/]/).filter(Boolean).at(-1) || selected.subject
    if (subject && selected.item.category === 'workspace' && !subject.endsWith('/')) subject += '/'
  }
  const summaryParts = CATEGORY_ORDER.flatMap((category) => {
    const count = presentation.categoryCounts[category]
    const key = presentation.outcomes.length ? SUMMARY_KEYS[category] : COMPLETED_KEYS[category]
    return count ? [{ category, label: formatTranslation(t, key, { count }) }] : []
  })
  const visibleSummaryParts = expanded ? summaryParts : summaryParts.slice(0, 3)
  const overflowVariants = expanded
    ? []
    : [
        { size: 'wide', count: summaryParts.length - 3 },
        { size: 'compact', count: summaryParts.length - 2 }
      ].filter((variant) => variant.count > 0)
  const label = selected
    ? [selectedAction, subject].filter(Boolean).join(' ')
    : summaryParts.map((part) => part.label).join(' · ')
  const category =
    selected?.item.category ??
    SUMMARY_ICON_ORDER.find((candidate) => presentation.categoryCounts[candidate]) ??
    'read'
  const Icon = CATEGORY_ICONS[category]
  const fullLabel = selected ? [selectedAction, selected.subject].filter(Boolean).join(' ') : label
  const counts = selected?.item.category === 'edit' ? selected.editCounts : undefined
  return (
    <button
      className="basic-tool-activity__header"
      type="button"
      onClick={onToggle}
      aria-expanded={expanded}
      aria-controls={controls}
      aria-label={selected ? undefined : fullLabel}
      data-status={selected?.phase ?? presentation.outcomes[0] ?? 'completed'}
      data-active={selected?.isActive ? 'true' : 'false'}
      title={fullLabel}
    >
      <Icon aria-hidden="true" className="basic-tool-activity__icon" data-category={category} />
      <span
        className={`basic-tool-activity__label${selected?.isActive ? ' agent-running-text' : ''}`}
        data-summary={selected ? undefined : 'true'}
      >
        {selected ? (
          label
        ) : (
          <>
            {visibleSummaryParts.map((part, index) => (
              <span
                key={part.category}
                className="basic-tool-activity__summary-part"
                data-category={part.category}
                data-summary-index={index}
              >
                {index > 0 ? (
                  <span className="basic-tool-activity__summary-separator" aria-hidden="true">
                    {' · '}
                  </span>
                ) : null}
                {part.label}
              </span>
            ))}
            {overflowVariants.map((variant) => (
              <span
                key={variant.size}
                className={`basic-tool-activity__summary-overflow basic-tool-activity__summary-overflow--${variant.size}`}
                aria-hidden="true"
              >
                …
              </span>
            ))}
          </>
        )}
      </span>
      {counts ? (
        <span
          className="basic-tool-activity__counts file-change-activity__stats"
          aria-label={`+${counts.additions} -${counts.deletions}`}
        >
          <RollingLineCount
            className="file-change-activity__additions"
            sign="+"
            value={counts.additions}
          />
          <RollingLineCount
            className="file-change-activity__deletions"
            sign="-"
            value={counts.deletions}
          />
        </span>
      ) : null}
      <ChevronDown
        aria-hidden="true"
        className="basic-tool-activity__chevron"
        data-expanded={expanded ? 'true' : 'false'}
      />
    </button>
  )
}
