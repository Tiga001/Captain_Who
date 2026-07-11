import { SquareTerminal } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import { getToolCallLabel, type SettledToolStatus } from './toolActivityUtils'

interface RunCommandToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

export interface RunCommandToolActivityGroupItem extends RunCommandToolActivityProps {}

interface RunCommandToolActivityGroupProps {
  items: RunCommandToolActivityGroupItem[]
}

type RunCommandStatus = 'running' | 'completed' | 'failed' | 'rejected' | 'cancelled'

function getObjectValue(value: unknown) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  return value as Record<string, unknown>
}

function getStringField(value: unknown, key: string) {
  const objectValue = getObjectValue(value)
  const fieldValue = objectValue?.[key]
  return typeof fieldValue === 'string' && fieldValue.trim() ? fieldValue.trim() : ''
}

function getRunCommandDetails(call: AgentToolCall) {
  return {
    command: getStringField(call.args, 'command'),
    reason: getStringField(call.args, 'reason') || call.reason || ''
  }
}

function isRejectedResult(result: AgentToolResult | undefined) {
  const resultValue = getObjectValue(result?.result)
  return resultValue?.status === 'rejected'
}

function getRejectedMessage(result: AgentToolResult | undefined) {
  const resultValue = getObjectValue(result?.result)
  const message = resultValue?.message
  return typeof message === 'string' ? message.trim() : ''
}

function getCommandResult(result: AgentToolResult | undefined) {
  const resultValue = getObjectValue(result?.result)
  if (resultValue?.status === 'rejected') return null
  if (!resultValue || typeof resultValue.command !== 'string') return null

  return {
    command: resultValue.command.trim(),
    stdout: typeof resultValue.stdout === 'string' ? resultValue.stdout : '',
    stderr: typeof resultValue.stderr === 'string' ? resultValue.stderr : '',
    error: typeof resultValue.error === 'string' ? resultValue.error : '',
    exitCode: typeof resultValue.exitCode === 'number' ? resultValue.exitCode : undefined,
    timedOut: resultValue.timedOut === true,
    cancelled: resultValue.cancelled === true
  }
}

function getCommandOutput(commandResult: ReturnType<typeof getCommandResult>) {
  if (!commandResult) return ''
  return [commandResult.stdout, commandResult.stderr, commandResult.error]
    .map((value) => value.trim())
    .filter(Boolean)
    .join('\n\n')
}

function getCommandStatus(
  commandResult: ReturnType<typeof getCommandResult>,
  ok: boolean | undefined,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  if (commandResult?.cancelled) return t('agent.command.cancelled')
  if (commandResult?.timedOut) return t('agent.command.timedOut')
  return ok === false ? t('agent.command.failedStatus') : t('agent.command.successStatus')
}

function getRunCommandStatus(item: RunCommandToolActivityGroupItem): RunCommandStatus {
  if (item.cancelled && !item.result) return 'cancelled'
  if (isRejectedResult(item.result)) return 'rejected'
  if (item.result?.ok === false) return 'failed'
  if (item.result) return 'completed'
  if (item.settledStatus) return item.settledStatus
  return 'running'
}

function getGroupLabel(
  items: RunCommandToolActivityGroupItem[],
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  const counts = items.reduce(
    (currentCounts, item) => {
      currentCounts[getRunCommandStatus(item)] += 1
      return currentCounts
    },
    { cancelled: 0, completed: 0, failed: 0, rejected: 0, running: 0 }
  )

  if (
    counts.running === 0 &&
    counts.failed === 0 &&
    counts.rejected === 0 &&
    counts.cancelled === 0
  ) {
    return formatTranslation(t, 'agent.command.groupCompleted', { count: String(items.length) })
  }

  const summaryParts = [
    counts.running > 0
      ? formatTranslation(t, 'agent.command.groupRunningCount', { count: String(counts.running) })
      : '',
    counts.completed > 0
      ? formatTranslation(t, 'agent.command.groupSucceededCount', {
          count: String(counts.completed)
        })
      : '',
    counts.failed > 0
      ? formatTranslation(t, 'agent.command.groupFailedCount', { count: String(counts.failed) })
      : '',
    counts.rejected > 0
      ? formatTranslation(t, 'agent.command.groupRejectedCount', { count: String(counts.rejected) })
      : '',
    counts.cancelled > 0
      ? formatTranslation(t, 'agent.command.groupCancelledCount', {
          count: String(counts.cancelled)
        })
      : ''
  ].filter(Boolean)

  return [
    formatTranslation(t, 'agent.command.groupProcessedTotal', { count: String(items.length) }),
    ...summaryParts
  ].join(t('agent.separator'))
}

export function RunCommandToolActivity({
  cancelled = false,
  call,
  result,
  settledStatus
}: RunCommandToolActivityProps) {
  const { t } = useFrontendConfig()
  const details = getRunCommandDetails(call)
  const rejected = isRejectedResult(result)
  const rejectedMessage = getRejectedMessage(result)
  const commandResult = getCommandResult(result)
  const command = commandResult?.command || details.command
  const commandOutput = getCommandOutput(commandResult)
  const commandFailed = result?.ok === false
  const status = getRunCommandStatus({ cancelled, call, result, settledStatus })
  const hasDetails = Boolean(
    details.reason || command || rejectedMessage || result?.error || commandResult
  )
  const isPending = status === 'running'
  const iconBadge = (() => {
    if (rejected) {
      return (
        <span
          aria-hidden="true"
          className="agent-activity__icon-mark agent-activity__icon-mark--slash"
        />
      )
    }
    if (status === 'failed') {
      return (
        <span
          aria-hidden="true"
          className="agent-activity__icon-mark agent-activity__icon-mark--bang"
        >
          !
        </span>
      )
    }
    return null
  })()
  const label = (() => {
    if (rejected) return t('agent.command.rejected')
    if (commandResult) {
      return formatTranslation(
        t,
        result?.ok === false ? 'agent.command.failed' : 'agent.command.completed',
        {
          command
        }
      )
    }
    if (status === 'completed') return formatTranslation(t, 'agent.command.completed', { command })
    if (status === 'failed') return formatTranslation(t, 'agent.command.failed', { command })
    return getToolCallLabel(call, result, t, { cancelled, settledStatus })
  })()

  return (
    <AgentActivityDisclosure
      className="agent-activity--run-command"
      hasDetails={hasDetails}
      icon={SquareTerminal}
      iconBadge={iconBadge}
      iconBadgeTone={rejected ? 'blocked' : status === 'failed' ? 'danger' : undefined}
      isPending={isPending}
      label={label}
    >
      {hasDetails && (
        <div className="agent-activity__details run-command-activity__details">
          {commandResult ? (
            <>
              {details.reason && <p className="run-command-activity__reason">{details.reason}</p>}
              <div className="run-command-shell" role="group" aria-label={t('agent.command.shell')}>
                <div className="run-command-shell__title">{t('agent.command.shell')}</div>
                {command && <pre className="run-command-shell__command">$ {command}</pre>}
                <pre className="run-command-shell__output">
                  {commandOutput || t('agent.command.noOutput')}
                </pre>
                <div
                  className="run-command-shell__status"
                  data-status={commandFailed ? 'failed' : 'succeeded'}
                >
                  <span aria-hidden="true">{commandFailed ? '×' : '✓'}</span>
                  <span>{getCommandStatus(commandResult, result?.ok, t)}</span>
                </div>
              </div>
            </>
          ) : (
            <>
              {details.reason && <p className="run-command-activity__reason">{details.reason}</p>}
              {command && <pre>{command}</pre>}
            </>
          )}
          {rejectedMessage && (
            <>
              <span>{t('agent.command.rejectReason')}</span>
              <p>{rejectedMessage}</p>
            </>
          )}
          {result?.error && !commandResult && (
            <>
              <span>{t('agent.detail.error')}</span>
              <pre>{result.error}</pre>
            </>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}

export function RunCommandToolActivityGroup({ items }: RunCommandToolActivityGroupProps) {
  const { t } = useFrontendConfig()
  if (items.length === 0) return null
  if (items.length === 1) {
    const item = items[0]
    return (
      <RunCommandToolActivity
        cancelled={item.cancelled}
        call={item.call}
        result={item.result}
        settledStatus={item.settledStatus}
      />
    )
  }

  const isPending = items.some((item) => getRunCommandStatus(item) === 'running')

  return (
    <AgentActivityDisclosure
      className="agent-activity--run-command"
      hasDetails
      icon={SquareTerminal}
      isPending={isPending}
      label={getGroupLabel(items, t)}
    >
      <div className="agent-activity__details run-command-activity__details run-command-activity__details--group">
        {items.map((item) => (
          <RunCommandToolActivity
            cancelled={item.cancelled}
            call={item.call}
            key={item.call.id}
            result={item.result}
            settledStatus={item.settledStatus}
          />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
