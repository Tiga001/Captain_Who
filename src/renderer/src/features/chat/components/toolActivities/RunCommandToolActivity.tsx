import { Check, Copy, SquareTerminal } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import type { ChatCommandOutputPreview } from '../../chatTypes'
import { copyTextToClipboard } from '../chatMessageItemUtils'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import { getToolCallLabel, getToolDisplayName, type SettledToolStatus } from './toolActivityUtils'

interface RunCommandToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
  liveOutput?: ChatCommandOutputPreview
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

export interface RunCommandToolActivityGroupItem extends RunCommandToolActivityProps {}

interface RunCommandToolActivityGroupProps {
  items: RunCommandToolActivityGroupItem[]
}

type RunCommandStatus = 'running' | 'completed' | 'failed' | 'rejected' | 'cancelled'

const COPIED_INDICATOR_MS = 1300

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

function getRunningCommandReceipt(result: AgentToolResult | undefined) {
  const resultValue = getObjectValue(result?.result)
  if (
    result?.ok !== true ||
    resultValue?.status !== 'running' ||
    typeof resultValue.sessionId !== 'string' ||
    !resultValue.sessionId.trim()
  ) {
    return null
  }

  const latestSequence = resultValue.latestSequence
  return {
    sessionId: resultValue.sessionId.trim(),
    output: typeof resultValue.output === 'string' ? resultValue.output : '',
    latestSequence:
      typeof latestSequence === 'number' && Number.isSafeInteger(latestSequence)
        ? Math.max(0, latestSequence)
        : 0
  }
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

function getLiveCommandOutput(preview: ChatCommandOutputPreview | undefined, afterSequence = 0) {
  return (
    preview?.chunks
      .filter((chunk) => chunk.sequence > afterSequence)
      .map((chunk) => chunk.output)
      .join('') ?? ''
  )
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
  if (getRunningCommandReceipt(item.result)) return 'running'
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
  liveOutput,
  result,
  settledStatus
}: RunCommandToolActivityProps) {
  const { t } = useFrontendConfig()
  const details = getRunCommandDetails(call)
  const rejected = isRejectedResult(result)
  const rejectedMessage = getRejectedMessage(result)
  const runningReceipt = getRunningCommandReceipt(result)
  const commandResult = getCommandResult(result)
  const command = commandResult?.command || details.command
  const commandOutput = getCommandOutput(commandResult)
  const liveCommandOutput = getLiveCommandOutput(liveOutput, runningReceipt?.latestSequence)
  const runningCommandOutput = `${runningReceipt?.output ?? ''}${liveCommandOutput}`
  const copyableOutput = commandOutput || runningCommandOutput
  const commandFailed = result?.ok === false
  const status = getRunCommandStatus({ cancelled, call, result, settledStatus })
  const hasDetails = Boolean(
    command || rejectedMessage || result?.error || commandResult || runningCommandOutput
  )
  const isPending = status === 'running'
  const [copied, setCopied] = useState(false)
  const outputRef = useRef<HTMLPreElement>(null)
  const keepLiveOutputPinnedRef = useRef(true)
  useEffect(() => {
    if (!copied) return undefined
    const timerId = window.setTimeout(() => setCopied(false), COPIED_INDICATOR_MS)
    return () => window.clearTimeout(timerId)
  }, [copied])
  useLayoutEffect(() => {
    if (!isPending || !runningCommandOutput || !keepLiveOutputPinnedRef.current) return
    const output = outputRef.current
    if (output) output.scrollTop = output.scrollHeight
  }, [isPending, runningCommandOutput])
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
  const statusLabel = (() => {
    if (rejected) return t('agent.command.rejected')
    if (runningReceipt) {
      return formatTranslation(t, 'agent.tool.running', {
        tool: getToolDisplayName(call.tool, t)
      })
    }
    if (commandResult) {
      return t(result?.ok === false ? 'agent.command.failed' : 'agent.command.completed')
    }
    if (status === 'completed') return t('agent.command.completed')
    if (status === 'failed') return t('agent.command.failed')
    return getToolCallLabel(call, result, t, { cancelled, settledStatus })
  })()
  const label = details.reason ? `${statusLabel} ${details.reason}` : statusLabel

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
          {commandResult || runningCommandOutput || isPending ? (
            <div className="run-command-shell" role="group" aria-label={t('agent.command.shell')}>
              <div className="run-command-shell__title">{t('agent.command.shell')}</div>
              {command && <pre className="run-command-shell__command">$ {command}</pre>}
              <div className="run-command-shell__output-region">
                <pre
                  className={[
                    'run-command-shell__output',
                    isPending ? 'run-command-shell__output--live' : ''
                  ]
                    .filter(Boolean)
                    .join(' ')}
                  onScroll={(event) => {
                    const output = event.currentTarget
                    keepLiveOutputPinnedRef.current =
                      output.scrollHeight - output.scrollTop - output.clientHeight < 24
                  }}
                  ref={outputRef}
                >
                  {copyableOutput ||
                    t(isPending ? 'agent.command.waitingForOutput' : 'agent.command.noOutput')}
                </pre>
                {copyableOutput && (
                  <button
                    aria-label={
                      copied ? t('agent.command.outputCopied') : t('agent.command.copyOutput')
                    }
                    className="run-command-shell__copy"
                    onClick={() => {
                      void copyTextToClipboard(copyableOutput)
                        .then(() => setCopied(true))
                        .catch(() => setCopied(false))
                    }}
                    title={copied ? t('agent.command.outputCopied') : t('agent.command.copyOutput')}
                    type="button"
                  >
                    {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
                  </button>
                )}
              </div>
              <div
                className="run-command-shell__status"
                data-status={isPending ? 'running' : commandFailed ? 'failed' : 'succeeded'}
                aria-live="polite"
              >
                <span aria-hidden="true">{isPending ? '•' : commandFailed ? '×' : '✓'}</span>
                <span>
                  {isPending
                    ? t('agent.command.runningStatus')
                    : getCommandStatus(commandResult, result?.ok, t)}
                </span>
              </div>
            </div>
          ) : (
            command && <pre>{command}</pre>
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
        liveOutput={item.liveOutput}
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
            liveOutput={item.liveOutput}
            result={item.result}
            settledStatus={item.settledStatus}
          />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
