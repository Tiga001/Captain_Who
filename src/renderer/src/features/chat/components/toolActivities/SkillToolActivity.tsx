// Renderer UI for activated Skill summaries and structured Skill-owned resource/script activity.

import {
  BookOpenText,
  CheckCircle2,
  FileStack,
  LayoutTemplate,
  LoaderCircle,
  WandSparkles,
  XCircle
} from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { getSkillPresentation } from '../../../skills/skillPresentation'
import type { ChatAgentRunView } from '../../chatTypes'
import {
  getActivatedSkills,
  getSkillScriptActivityView,
  type AgentActivityStatus,
  type SkillResourceActivityItem,
  type SkillResourceActivityKind
} from '../../skillOfficeActivity'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

function aggregateStatus(items: SkillResourceActivityItem[]): AgentActivityStatus {
  if (items.some((item) => item.status === 'waiting')) return 'waiting'
  if (items.some((item) => item.status === 'running')) return 'running'
  if (items.some((item) => item.status === 'conflict')) return 'conflict'
  if (items.some((item) => item.status === 'rejected')) return 'rejected'
  if (items.some((item) => item.status === 'failed')) return 'failed'
  if (items.some((item) => item.status === 'cancelled')) return 'cancelled'
  return 'completed'
}

function statusIcon(status: AgentActivityStatus) {
  if (status === 'running' || status === 'waiting') return LoaderCircle
  if (status === 'failed' || status === 'rejected' || status === 'conflict') return XCircle
  return CheckCircle2
}

function resourceLabel(
  kind: SkillResourceActivityKind,
  status: AgentActivityStatus,
  count: number,
  skillName: string | undefined,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  const name = skillName ?? t('agent.skill.unknown')
  const subject =
    kind === 'template'
      ? t('agent.skill.template')
      : kind === 'reference'
        ? t('agent.skill.reference')
        : t('agent.skill.capability')
  const action = kind === 'capability' ? t('agent.skill.actionUse') : t('agent.skill.actionRead')
  const key =
    status === 'waiting'
      ? 'agent.skill.resource.waiting'
      : status === 'running'
        ? 'agent.skill.resource.running'
        : status === 'cancelled'
          ? 'agent.skill.resource.cancelled'
          : status === 'failed' || status === 'conflict' || status === 'rejected'
            ? 'agent.skill.resource.failed'
            : count > 1
              ? 'agent.skill.resource.completedMany'
              : 'agent.skill.resource.completed'

  return formatTranslation(t, key, {
    action,
    count: String(count),
    skill: name,
    subject
  })
}

export function SkillLoadActivity({ run }: { run: ChatAgentRunView }) {
  const { t } = useFrontendConfig()
  const skills = getActivatedSkills(run)
  if (skills.length === 0) return null
  const label =
    skills.length === 1
      ? t('agent.skill.loaded')
      : formatTranslation(t, 'agent.skill.loadedMany', { count: String(skills.length) })

  return (
    <AgentActivityDisclosure
      className="agent-activity--skill"
      hasDetails
      icon={WandSparkles}
      label={label}
    >
      <div className="agent-activity__details skill-activity__details">
        {skills.map((skill) => (
          <p key={skill.id}>
            {formatTranslation(t, 'agent.skill.loadedItem', {
              skill: getSkillPresentation(skill, t).name
            })}
          </p>
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}

export function SkillResourceActivityGroup({
  items,
  kind
}: {
  items: SkillResourceActivityItem[]
  kind: SkillResourceActivityKind
}) {
  const { t } = useFrontendConfig()
  if (items.length === 0) return null
  const status = aggregateStatus(items)
  const Icon = statusIcon(status)
  const skill = items.find((item) => item.skill)?.skill
  const skillName = skill ? getSkillPresentation(skill, t).name : undefined
  const ResourceIcon =
    kind === 'template' ? LayoutTemplate : kind === 'reference' ? BookOpenText : FileStack
  const hasDetails = items.some((item) => item.detail || item.resourceName)

  return (
    <AgentActivityDisclosure
      className="agent-activity--skill-resource"
      hasDetails={hasDetails}
      icon={status === 'completed' ? ResourceIcon : Icon}
      isPending={status === 'running' || status === 'waiting'}
      label={resourceLabel(kind, status, items.length, skillName, t)}
    >
      {hasDetails && (
        <div className="agent-activity__details skill-activity__details">
          {items.map((item) => (
            <p key={item.resourceKey}>{item.detail || item.resourceName}</p>
          ))}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}

export function SkillScriptToolActivity({
  run,
  callId,
  settledStatus
}: {
  run: ChatAgentRunView
  callId: string
  settledStatus?: SettledToolStatus
}) {
  const { t } = useFrontendConfig()
  const call = run.toolCalls.find((candidate) => candidate.id === callId)
  if (!call) return null
  const view = getSkillScriptActivityView(run, call, settledStatus)
  if (!view) return null
  const skillName = view.skill ? getSkillPresentation(view.skill, t).name : t('agent.skill.unknown')
  const labelKey =
    view.status === 'waiting'
      ? 'agent.skill.script.waiting'
      : view.status === 'running'
        ? 'agent.skill.script.running'
        : view.status === 'cancelled'
          ? 'agent.skill.script.cancelled'
          : view.status === 'failed' || view.status === 'conflict' || view.status === 'rejected'
            ? 'agent.skill.script.failed'
            : 'agent.skill.script.completed'

  return (
    <AgentActivityDisclosure
      className="agent-activity--skill-script"
      hasDetails={Boolean(view.detail)}
      icon={statusIcon(view.status)}
      isPending={view.status === 'running' || view.status === 'waiting'}
      label={formatTranslation(t, labelKey, { skill: skillName })}
    >
      {view.detail && (
        <div className="agent-activity__details skill-activity__details">
          <p>{view.detail}</p>
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
