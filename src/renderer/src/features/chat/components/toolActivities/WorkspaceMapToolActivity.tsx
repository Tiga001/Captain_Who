import { FolderOpen } from 'lucide-react'
import type { AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface WorkspaceMapToolActivityProps {
  cancelled?: boolean
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

type WorkspaceMapStatus = 'running' | 'completed' | 'failed' | 'cancelled'

const STATUS_LABELS: Record<WorkspaceMapStatus, TranslationKey> = {
  running: 'agent.workspaceMap.running',
  completed: 'agent.workspaceMap.completed',
  failed: 'agent.workspaceMap.failed',
  cancelled: 'agent.workspaceMap.cancelled'
}

function getWorkspaceMapStatus(
  cancelled: boolean,
  result: AgentToolResult | undefined,
  settledStatus?: SettledToolStatus
): WorkspaceMapStatus {
  if (cancelled && !result) return 'cancelled'
  if (result?.ok === false) return 'failed'
  if (result) return 'completed'
  if (settledStatus) return settledStatus
  return 'running'
}

export function WorkspaceMapToolActivity({
  cancelled = false,
  result,
  settledStatus
}: WorkspaceMapToolActivityProps) {
  const { t } = useFrontendConfig()
  const status = getWorkspaceMapStatus(cancelled, result, settledStatus)
  const hasDetails = status !== 'running'

  return (
    <AgentActivityDisclosure
      className="agent-activity--workspace-map"
      hasDetails={hasDetails}
      icon={FolderOpen}
      isPending={status === 'running'}
      label={t(STATUS_LABELS[status])}
    >
      {hasDetails && (
        <div className="agent-activity__details workspace-map-activity__details">
          {result?.error ? (
            <p>{result.error}</p>
          ) : (
            <p>
              {status === 'cancelled'
                ? t('agent.workspaceMap.cancelledDetail')
                : t('agent.workspaceMap.listedFiles')}
            </p>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
