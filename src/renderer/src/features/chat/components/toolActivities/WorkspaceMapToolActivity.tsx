import { FolderOpen } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface WorkspaceMapToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function getFolderName(call: AgentToolCall, result: AgentToolResult | undefined): string {
  const output = isRecord(result?.result) ? result.result : undefined
  const workspace = isRecord(output?.workspace) ? output.workspace : undefined
  const resultPath = workspace?.focusPath
  const requestedPath = isRecord(call.args) ? call.args.focusPath : undefined
  const focusPath = typeof resultPath === 'string' && resultPath.trim() ? resultPath : requestedPath
  if (typeof focusPath !== 'string') return ''
  const path = focusPath.trim().replace(/\\/g, '/')
  const withoutTrailingSlash = path.replace(/\/+$/, '')
  if (!path || withoutTrailingSlash === '.') return ''
  return withoutTrailingSlash.split('/').filter(Boolean).at(-1) || path
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
  call,
  result,
  settledStatus
}: WorkspaceMapToolActivityProps) {
  const { t } = useFrontendConfig()
  const status = getWorkspaceMapStatus(cancelled, result, settledStatus)
  const folder = getFolderName(call, result) || t('agent.workspaceMap.workspace')

  return (
    <AgentActivityDisclosure
      className="agent-activity--workspace-map"
      hasDetails={false}
      icon={FolderOpen}
      isPending={status === 'running'}
      label={formatTranslation(t, STATUS_LABELS[status], { folder })}
    />
  )
}
