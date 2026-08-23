import { CircleCheckBig, CircleStop, CircleX, PackageSearch, WandSparkles } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { ChatAgentRunView } from '../../chatTypes'
import {
  SkillInstallationPreviewDetails,
  SkillInstallationSummaryDetails,
  skillInstallationPlainText
} from '../SkillInstallationPreviewDetails'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface SkillInstallationToolActivityProps {
  call: AgentToolCall
  result?: AgentToolResult
  run: ChatAgentRunView
  settledStatus?: SettledToolStatus
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

export function SkillInstallationToolActivity({
  call,
  result,
  run,
  settledStatus
}: SkillInstallationToolActivityProps) {
  const { t } = useFrontendConfig()
  const installation = run.skillInstallations?.find((item) => item.action.id === call.id)

  if (call.tool === 'skills_prepare_install') {
    const output = record(result?.result)
    const status = output?.status
    const failed = result?.ok === false || status === 'invalid' || settledStatus === 'failed'
    const identified = status === 'ready'
    const needsSelection = status === 'needsSelection'
    const pending = !result && !settledStatus
    const label = failed
      ? t('agent.skillInstallation.inspectFailed')
      : identified
        ? t('agent.skillInstallation.identified')
        : needsSelection
          ? t('agent.skillInstallation.selectionRequired')
          : pending
            ? t('agent.skillInstallation.inspecting')
            : t('agent.skillInstallation.inspected')
    const Icon = failed
      ? CircleX
      : identified
        ? WandSparkles
        : needsSelection
          ? CircleCheckBig
          : PackageSearch
    const description = skillInstallationPlainText(output?.description, '')
    const hasSummary = identified && Boolean(output)
    const hasDetails = hasSummary || Boolean(result?.error)

    return (
      <AgentActivityDisclosure
        className="agent-activity--skill-installation"
        hasDetails={hasDetails}
        icon={Icon}
        isPending={pending}
        label={label}
      >
        {hasDetails ? (
          <div className="agent-activity__details agent-skill-installation-activity__inspection">
            {hasSummary ? (
              <SkillInstallationSummaryDetails
                description={description}
                resourceSummary={output?.resourceSummary}
                sourceSummary={output?.sourceSummary}
              />
            ) : null}
            {result?.error ? <p data-tone="danger">{result.error}</p> : null}
          </div>
        ) : null}
      </AgentActivityDisclosure>
    )
  }

  const status = installation?.status
  const failed =
    result?.ok === false ||
    settledStatus === 'failed' ||
    status === 'failed' ||
    status === 'uncertain' ||
    status === 'expired'
  const cancelled = settledStatus === 'cancelled'
  const stopped = status === 'rejected' || cancelled
  const pending =
    !failed &&
    !stopped &&
    (status === 'waiting_for_approval' ||
      status === 'installing' ||
      (!status && !result && !settledStatus))
  const label =
    status === 'uncertain'
      ? t('agent.skillInstallation.uncertain')
      : status === 'expired'
        ? t('agent.skillInstallation.prepareExpired')
        : status === 'rejected'
          ? t('agent.skillInstallation.rejected')
          : cancelled
            ? t('agent.skillInstallation.cancelled')
            : failed
              ? t('agent.skillInstallation.installFailed')
              : status === 'installed'
                ? t('agent.skillInstallation.installed')
                : status === 'already_installed'
                  ? t('agent.skillInstallation.alreadyInstalled')
                  : status === 'waiting_for_approval'
                    ? t('agent.skillInstallation.waitingApproval')
                    : t('agent.skillInstallation.installing')
  const Icon = failed
    ? CircleX
    : stopped
      ? CircleStop
      : status === 'installed' || status === 'already_installed'
        ? WandSparkles
        : PackageSearch

  return (
    <AgentActivityDisclosure
      className="agent-activity--skill-installation"
      hasDetails={Boolean(installation || result?.error)}
      icon={Icon}
      isPending={pending}
      label={label}
    >
      {installation ? (
        <div className="agent-activity__details">
          <SkillInstallationPreviewDetails preview={installation.action.preview} />
          {result?.error ? <p data-tone="danger">{result.error}</p> : null}
        </div>
      ) : result?.error ? (
        <div className="agent-activity__details">
          <p data-tone="danger">{result.error}</p>
        </div>
      ) : null}
    </AgentActivityDisclosure>
  )
}
