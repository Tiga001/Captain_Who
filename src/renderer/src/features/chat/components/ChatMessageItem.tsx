import { useEffect, useId, useMemo, useRef, useState } from 'react'
import {
  AlertTriangle,
  ChevronDown,
  Check,
  Copy,
  Database,
  LoaderCircle,
  Pencil,
  Split
} from 'lucide-react'
import type {
  AgentProposedAction,
  AgentToolCall,
  AgentToolResult,
  AgentUsage
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../config/translationFormat'
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatFileWritePreview,
  ChatMessage,
  ChatReadActivityKind
} from '../chatTypes'
import { getReadActivityKindForTool, isReadActivityTool } from '../agentReadActivities'
import { getUniqueWebSearchSources } from '../agentWebSearch'
import { stripAttachmentSummary } from '../chatAttachments'
import {
  getAttachmentBadgeLabel,
  getAttachmentExtension,
  getAttachmentIcon,
  getAttachmentPreviewUrl
} from '../attachmentDisplay'
import { loadAttachmentImage } from '../../storage/storageClient'
import { ChatMarkdown } from './ChatMarkdown'
import { EditSummaryCard } from './EditSummaryCard'
import { useImagePreview, useImagePreviewNotice } from './ImagePreview'
import { AgentToolActivity } from './toolActivities/AgentToolActivity'
import { ContextCompactionActivity } from './toolActivities/ContextCompactionActivity'
import {
  ConversationHistoryToolActivity,
  type ConversationHistoryActivityItem
} from './toolActivities/ConversationHistoryToolActivity'
import {
  FileWriteToolActivityGroup,
  type FileWriteToolActivityGroupItem
} from './toolActivities/FileWriteToolActivity'
import {
  ApplyPatchToolActivityGroup,
  type ApplyPatchToolActivityGroupItem
} from './toolActivities/ApplyPatchToolActivity'
import {
  ReadToolActivityGroup,
  type ReadToolActivityGroupItem
} from './toolActivities/ReadToolActivity'
import {
  RunCommandToolActivityGroup,
  type RunCommandToolActivityGroupItem
} from './toolActivities/RunCommandToolActivity'
import {
  getSearchKind,
  isSearchTool,
  SearchToolActivityGroup,
  type SearchKind,
  type SearchToolActivityGroupItem
} from './toolActivities/SearchToolActivity'
import type { SettledToolStatus } from './toolActivities/toolActivityUtils'
import {
  WebSearchToolActivityGroup,
  type WebSearchToolActivityGroupItem
} from './toolActivities/WebSearchToolActivity'
import { AssistantSources } from './toolActivities/WebSearchSources'

const ACTIVE_STREAMING_GRACE_MS = 1200
const FILE_WRITE_ACTIVITY_GRACE_MS = 2000
const COPIED_INDICATOR_MS = 1300

interface ChatMessageItemProps {
  editSelectedModelAvailable?: boolean
  editSelectedModelSupportsImage?: boolean
  isLastAssistantMessage?: boolean
  message: ChatMessage
  projectId?: string | null
  onApprove?: (
    messageId: string,
    action: AgentProposedAction,
    options?: { rememberForRun?: boolean }
  ) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onEditSubmit?: (messageId: string, content: string) => void | Promise<void>
  onContinueInNewTask?: (messageId: string) => void | Promise<void>
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  showTokenUsageDetails: boolean
}

type RenderableTimelineItem =
  | ChatAgentTimelineItem
  | {
      id: string
      type: 'read_group'
      kind: ChatReadActivityKind
      callIds: string[]
    }
  | {
      id: string
      type: 'search_group'
      kind: SearchKind
      callIds: string[]
    }
  | {
      id: string
      type: 'web_activity_group'
      kind: 'search' | 'fetch'
      callIds: string[]
    }
  | {
      id: string
      type: 'run_command_group'
      callIds: string[]
    }
  | {
      id: string
      type: 'apply_patch_group'
      callIds: string[]
    }
  | {
      id: string
      type: 'write_file_group'
      callIds: string[]
    }
  | {
      id: string
      type: 'conversation_history_group'
      callIds: string[]
    }

function formatMessageTime(timestamp: number | undefined, language: string, t: Translate) {
  if (!timestamp) return ''
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return ''

  const time = `${date.getHours()}:${date.getMinutes().toString().padStart(2, '0')}`
  const today = new Date()
  today.setHours(0, 0, 0, 0)

  const messageDay = new Date(date)
  messageDay.setHours(0, 0, 0, 0)

  const dayDistance = Math.floor((today.getTime() - messageDay.getTime()) / 86_400_000)
  if (dayDistance <= 0) return time
  if (dayDistance === 1) return `${t('chat.yesterday')} ${time}`
  if (dayDistance <= 7) {
    return `${new Intl.DateTimeFormat(language, { weekday: 'long' }).format(date)} ${time}`
  }

  return `${new Intl.DateTimeFormat(language, { month: 'short', day: 'numeric' }).format(date)} ${time}`
}

function formatUsageTokenCount(value: unknown, language: string) {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  return new Intl.NumberFormat(language).format(value)
}

function getUsageRows(usage: AgentUsage | undefined, language: string, t: Translate) {
  if (!usage) return []

  return [
    {
      label: t('chat.usageInputTokens'),
      value: formatUsageTokenCount(usage.inputTokens, language)
    },
    {
      label: t('chat.usageOutputTokens'),
      value: formatUsageTokenCount(usage.outputTokens, language)
    },
    {
      label: t('chat.usageTotalTokens'),
      value: formatUsageTokenCount(usage.totalTokens, language)
    },
    {
      label: t('chat.usageCachedInputTokens'),
      value: formatUsageTokenCount(usage.cachedInputTokens, language)
    },
    {
      label: t('chat.usageCacheCreationInputTokens'),
      value: formatUsageTokenCount(usage.cacheCreationInputTokens, language)
    }
  ].filter((row): row is { label: string; value: string } => row.value !== null)
}

async function copyTextToClipboard(content: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(content)
    return
  }

  const textarea = document.createElement('textarea')
  textarea.value = content
  textarea.setAttribute('readonly', '')
  textarea.style.position = 'fixed'
  textarea.style.top = '-1000px'
  textarea.style.opacity = '0'
  document.body.appendChild(textarea)
  textarea.select()
  document.execCommand('copy')
  document.body.removeChild(textarea)
}

function getToolResult(run: ChatAgentRunView, callId: string): AgentToolResult | undefined {
  return run.toolResults.find((result) => result.callId === callId)
}

function getPreviousSuccessfulTodoResult(
  run: ChatAgentRunView,
  callId: string
): AgentToolResult | undefined {
  const currentCallIndex = run.toolCalls.findIndex((call) => call.id === callId)
  for (let index = currentCallIndex - 1; index >= 0; index -= 1) {
    const previousCall = run.toolCalls[index]
    if (previousCall.tool !== 'todo_update') continue
    const previousResult = getToolResult(run, previousCall.id)
    if (previousResult?.ok) return previousResult
  }
  return undefined
}

function isRunSettled(run: ChatAgentRunView) {
  return (
    run.status === 'completed' ||
    run.status === 'failed' ||
    run.status === 'cancelled' ||
    (run.status === 'idle' && Boolean(run.completedAt))
  )
}

function getSettledToolStatus(
  run: ChatAgentRunView,
  result: ReturnType<typeof getToolResult>
): SettledToolStatus | undefined {
  if (result || !isRunSettled(run)) return undefined
  if (run.status === 'failed') return 'failed'
  if (run.status === 'cancelled') return 'cancelled'
  if (run.status === 'completed' || (run.status === 'idle' && run.completedAt)) return 'completed'
  return undefined
}

function isTokenLimitFinishReason(finishReason: string | undefined) {
  if (!finishReason) return false
  const normalized = finishReason.toLowerCase()
  return (
    normalized === 'length' ||
    normalized === 'max_tokens' ||
    normalized === 'max_output_tokens' ||
    normalized.includes('max_token')
  )
}

function isThinkingPlaceholder(content: string) {
  return content.trim() === '正在思考...'
}

function hasDisplayableContent(content: string) {
  return Boolean(content.trim()) && !isThinkingPlaceholder(content)
}

function isTimelineItemRenderable(run: ChatAgentRunView, item: ChatAgentTimelineItem) {
  if (item.type === 'message') return Boolean(item.content.trim())
  if (item.type === 'tool_call')
    return run.toolCalls.some((candidate) => candidate.id === item.callId)
  return true
}

function getLastRenderableTimelineItem(run: ChatAgentRunView, timeline: ChatAgentTimelineItem[]) {
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const item = timeline[index]
    if (isTimelineItemRenderable(run, item)) {
      return item
    }
  }

  return undefined
}

function getLastMessageTimelineContent(timeline: ChatAgentTimelineItem[]) {
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const item = timeline[index]
    if (item.type === 'message' && hasDisplayableContent(item.content)) {
      return item.content
    }
  }

  return ''
}

function getAssistantFinalContent(message: ChatMessage) {
  const timelineContent = getLastMessageTimelineContent(message.agentRun?.timeline ?? [])
  const content = timelineContent || message.content
  return isThinkingPlaceholder(content) ? '' : content
}

function getUserVisibleContent(message: ChatMessage) {
  if (message.role !== 'user' || !message.attachments?.length) return message.content
  return stripAttachmentSummary(message.content, message.attachments)
}

function isBottomTimelineItemSpecificPendingStatus(
  run: ChatAgentRunView,
  item: ChatAgentTimelineItem | undefined
) {
  if (!item) return false

  if (item.type === 'tool_call') {
    return !getToolResult(run, item.callId)
  }

  if (item.type === 'context_compaction') {
    return item.status === 'running'
  }

  return false
}

function shouldShowThinkingActivity(run: ChatAgentRunView, timeline: ChatAgentTimelineItem[]) {
  if (isRunSettled(run)) return false

  const lastItem = getLastRenderableTimelineItem(run, timeline)
  return !isBottomTimelineItemSpecificPendingStatus(run, lastItem)
}

function hasRecentFileWriteActivity(run: ChatAgentRunView, now: number): boolean {
  if (isRunSettled(run)) return false
  return Boolean(
    run.fileWritePreviews?.some(
      (preview) => now - preview.receivedAt <= FILE_WRITE_ACTIVITY_GRACE_MS
    )
  )
}

function hasCollapsibleTimelineContent(run: ChatAgentRunView, timeline: ChatAgentTimelineItem[]) {
  return timeline.some((item) => item.type !== 'message' && isTimelineItemRenderable(run, item))
}

function getReadKindForCall(run: ChatAgentRunView, call: AgentToolCall) {
  if (!isReadActivityTool(call.tool)) return undefined
  return (
    run.readActivities?.find((activity) => activity.callId === call.id)?.kind ??
    getReadActivityKindForTool(call.tool)
  )
}

function groupTimelineItems(
  run: ChatAgentRunView,
  timeline: ChatAgentTimelineItem[]
): RenderableTimelineItem[] {
  return timeline.reduce<RenderableTimelineItem[]>((items, item) => {
    if (item.type !== 'tool_call') return [...items, item]

    const call = run.toolCalls.find((candidate) => candidate.id === item.callId)
    if (!call) return [...items, item]

    if (call.tool === 'conversation_history') {
      const previousItem = items[items.length - 1]
      if (previousItem?.type === 'conversation_history_group') {
        return [
          ...items.slice(0, -1),
          {
            ...previousItem,
            callIds: [...previousItem.callIds, item.callId]
          }
        ]
      }

      return [
        ...items,
        {
          id: `conversation-history-group-${item.callId}`,
          type: 'conversation_history_group',
          callIds: [item.callId]
        }
      ]
    }

    if (call.tool === 'web_search' || call.tool === 'web_fetch') {
      const kind = call.tool === 'web_fetch' ? 'fetch' : 'search'
      const previousItem = items[items.length - 1]
      if (previousItem?.type === 'web_activity_group' && previousItem.kind === kind) {
        return [
          ...items.slice(0, -1),
          {
            ...previousItem,
            callIds: [...previousItem.callIds, item.callId]
          }
        ]
      }

      return [
        ...items,
        {
          id: `web-${kind}-group-${item.callId}`,
          type: 'web_activity_group',
          kind,
          callIds: [item.callId]
        }
      ]
    }

    if (call.tool === 'write_file') {
      let existingIndex = -1
      for (let index = items.length - 1; index >= 0; index -= 1) {
        const candidate = items[index]
        if (candidate.type === 'message') break
        if (candidate.type === 'write_file_group') {
          existingIndex = index
          break
        }
      }
      if (existingIndex >= 0) {
        const existing = items[existingIndex]
        if (existing.type !== 'write_file_group') return [...items, item]
        return items.map((candidate, index) =>
          index === existingIndex
            ? { ...existing, callIds: [...existing.callIds, item.callId] }
            : candidate
        )
      }
      return [
        ...items,
        {
          id: `write-file-group-${item.callId}`,
          type: 'write_file_group',
          callIds: [item.callId]
        }
      ]
    }

    const readKind = getReadKindForCall(run, call)
    if (readKind) {
      const previousItem = items[items.length - 1]
      if (previousItem?.type === 'read_group' && previousItem.kind === readKind) {
        return [
          ...items.slice(0, -1),
          {
            ...previousItem,
            callIds: [...previousItem.callIds, item.callId]
          }
        ]
      }

      return [
        ...items,
        {
          id: `read-group-${item.callId}`,
          type: 'read_group',
          kind: readKind,
          callIds: [item.callId]
        }
      ]
    }

    if (isSearchTool(call.tool)) {
      const searchKind = getSearchKind(call)
      const previousItem = items[items.length - 1]
      if (previousItem?.type === 'search_group' && previousItem.kind === searchKind) {
        return [
          ...items.slice(0, -1),
          {
            ...previousItem,
            callIds: [...previousItem.callIds, item.callId]
          }
        ]
      }

      return [
        ...items,
        {
          id: `search-group-${item.callId}`,
          type: 'search_group',
          kind: searchKind,
          callIds: [item.callId]
        }
      ]
    }

    if (call.tool === 'run_command') {
      const previousItem = items[items.length - 1]
      if (previousItem?.type === 'run_command_group') {
        return [
          ...items.slice(0, -1),
          {
            ...previousItem,
            callIds: [...previousItem.callIds, item.callId]
          }
        ]
      }

      return [
        ...items,
        {
          id: `run-command-group-${item.callId}`,
          type: 'run_command_group',
          callIds: [item.callId]
        }
      ]
    }

    if (call.tool === 'apply_patch') {
      const previousItem = items[items.length - 1]
      if (previousItem?.type === 'apply_patch_group') {
        return [
          ...items.slice(0, -1),
          {
            ...previousItem,
            callIds: [...previousItem.callIds, item.callId]
          }
        ]
      }

      return [
        ...items,
        {
          id: `apply-patch-group-${item.callId}`,
          type: 'apply_patch_group',
          callIds: [item.callId]
        }
      ]
    }

    return [...items, item]
  }, [])
}

function getWriteFileDraftId(run: ChatAgentRunView, call: AgentToolCall): string | undefined {
  const args =
    call.args && typeof call.args === 'object' ? (call.args as Record<string, unknown>) : {}
  if (typeof args.draftId === 'string' && args.draftId) return args.draftId
  const result = getToolResult(run, call.id)?.result
  if (!result || typeof result !== 'object') return undefined
  const resultDraftId = (result as Record<string, unknown>).draftId
  if (typeof resultDraftId === 'string' && resultDraftId) return resultDraftId
  const draft = (result as Record<string, unknown>).draft
  if (!draft || typeof draft !== 'object') return undefined
  const draftId = (draft as Record<string, unknown>).draftId
  return typeof draftId === 'string' && draftId ? draftId : undefined
}

function getLatestWriteFilePreview(
  run: ChatAgentRunView,
  draftId: string | undefined
): ChatFileWritePreview | undefined {
  if (!draftId) return undefined
  return run.fileWritePreviews?.reduce<ChatFileWritePreview | undefined>((latest, preview) => {
    if (preview.draftId !== draftId) return latest
    return !latest || preview.receivedAt >= latest.receivedAt ? preview : latest
  }, undefined)
}

function getWriteFileGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): FileWriteToolActivityGroupItem[] {
  const itemsByDraft = new Map<string, FileWriteToolActivityGroupItem>()

  callIds.forEach((callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return
    const result = getToolResult(run, call.id)
    const draftId = getWriteFileDraftId(run, call)
    const itemKey = draftId ?? `pending-${call.id}`
    const existing = itemsByDraft.get(itemKey)
    const draft = draftId
      ? run.fileDrafts?.find((candidate) => candidate.draftId === draftId)
      : undefined
    const preview = getLatestWriteFilePreview(run, draftId)
    const draftIsUnsettled =
      draft && ['drafting', 'ready', 'waiting_approval', 'applying'].includes(draft.status)
    const settledStatus =
      draftIsUnsettled && isRunSettled(run)
        ? run.status === 'failed'
          ? 'failed'
          : 'cancelled'
        : getSettledToolStatus(run, result ?? existing?.result)
    itemsByDraft.set(itemKey, {
      call,
      draft,
      draftId: draftId ?? call.id,
      preview,
      result: result ?? existing?.result,
      settledStatus
    })
  })

  return [...itemsByDraft.values()]
}

function getReadGroupItems(run: ChatAgentRunView, callIds: string[]): ReadToolActivityGroupItem[] {
  return callIds.reduce<ReadToolActivityGroupItem[]>((items, callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return items
    const result = getToolResult(run, call.id)

    return [
      ...items,
      {
        activity: run.readActivities?.find((activity) => activity.callId === call.id),
        call,
        result,
        settledStatus: getSettledToolStatus(run, result)
      }
    ]
  }, [])
}

function getSearchGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): SearchToolActivityGroupItem[] {
  return callIds.reduce<SearchToolActivityGroupItem[]>((items, callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return items
    const result = getToolResult(run, call.id)
    const settledStatus = getSettledToolStatus(run, result)

    return [
      ...items,
      {
        call,
        cancelled: settledStatus === 'cancelled',
        result,
        settledStatus
      }
    ]
  }, [])
}

function getWebActivityGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): WebSearchToolActivityGroupItem[] {
  return callIds.reduce<WebSearchToolActivityGroupItem[]>((items, callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return items
    const result = getToolResult(run, call.id)

    return [
      ...items,
      {
        activity: run.webSearchActivities?.find((activity) => activity.callId === call.id),
        call,
        result,
        settledStatus: getSettledToolStatus(run, result)
      }
    ]
  }, [])
}

function getRunCommandGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): RunCommandToolActivityGroupItem[] {
  return callIds.reduce<RunCommandToolActivityGroupItem[]>((items, callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return items
    const result = getToolResult(run, call.id)
    const settledStatus = getSettledToolStatus(run, result)

    return [
      ...items,
      {
        call,
        cancelled: settledStatus === 'cancelled',
        result,
        settledStatus
      }
    ]
  }, [])
}

function getApplyPatchGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): ApplyPatchToolActivityGroupItem[] {
  return callIds.reduce<ApplyPatchToolActivityGroupItem[]>((items, callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return items
    const result = getToolResult(run, call.id)
    const settledStatus = getSettledToolStatus(run, result)

    return [
      ...items,
      {
        call,
        cancelled: settledStatus === 'cancelled',
        diff: run.diffs.find((candidate) => candidate.id === call.id),
        result,
        settledStatus
      }
    ]
  }, [])
}

function getConversationHistoryGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): ConversationHistoryActivityItem[] {
  return callIds.reduce<ConversationHistoryActivityItem[]>((items, callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return items
    const result = getToolResult(run, call.id)

    return [
      ...items,
      {
        call,
        result,
        settledStatus: getSettledToolStatus(run, result)
      }
    ]
  }, [])
}

function shouldShowAssistantActions(message: ChatMessage) {
  if (message.role !== 'assistant' || message.status !== 'sent') return false
  if (!getAssistantFinalContent(message).trim()) return false
  return (
    !message.agentRun ||
    message.agentRun.status === 'completed' ||
    message.agentRun.status === 'idle' ||
    message.agentRun.status === 'cancelled'
  )
}

function formatElapsedDuration(milliseconds: number) {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1000))

  if (totalSeconds < 60) {
    return `${totalSeconds}s`
  }

  const totalMinutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds % 60

  if (totalMinutes < 60) {
    return seconds > 0 ? `${totalMinutes}m ${seconds}s` : `${totalMinutes}m`
  }

  const hours = Math.floor(totalMinutes / 60)
  const minutes = totalMinutes % 60
  return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`
}

function AgentRunElapsedHeader({
  canToggle,
  collapsed,
  isThinking,
  label,
  onToggle
}: {
  canToggle: boolean
  collapsed: boolean
  isThinking: boolean
  label: string
  onToggle: () => void
}) {
  const content = (
    <>
      <span className={isThinking ? 'agent-running-text' : undefined}>{label}</span>
      {canToggle && (
        <ChevronDown
          aria-hidden="true"
          className="agent-run__elapsed-chevron"
          data-collapsed={collapsed ? 'true' : 'false'}
        />
      )}
    </>
  )

  if (!canToggle) {
    return <div className="agent-run__elapsed">{content}</div>
  }

  return (
    <button
      aria-expanded={!collapsed}
      className="agent-run__elapsed agent-run__elapsed-button"
      onClick={onToggle}
      type="button"
    >
      {content}
    </button>
  )
}

function UsageAction({ usage }: { usage: AgentUsage | undefined }) {
  const { language, t } = useFrontendConfig()
  const popoverId = useId()
  const rows = getUsageRows(usage, language, t)

  if (rows.length === 0) return null

  return (
    <div className="chat-message__usage">
      <button aria-describedby={popoverId} aria-label={t('chat.usage')} type="button">
        <Database aria-hidden="true" />
      </button>
      <div className="chat-message__usage-popover" id={popoverId} role="tooltip">
        <p className="chat-message__usage-title">{t('chat.usageTitle')}</p>
        <dl className="chat-message__usage-list">
          {rows.map((row) => (
            <div className="chat-message__usage-row" key={row.label}>
              <dt>{row.label}</dt>
              <dd>{row.value}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  )
}

function ChatMessageActions({
  canEdit = false,
  content,
  onEdit,
  onContinueInNewTask,
  showTokenUsageDetails,
  timestamp,
  usage
}: {
  canEdit?: boolean
  content: string
  onEdit?: () => void
  onContinueInNewTask?: () => void | Promise<void>
  showTokenUsageDetails: boolean
  timestamp: number | undefined
  usage?: AgentUsage
}) {
  const { language, t } = useFrontendConfig()
  const [copied, setCopied] = useState(false)
  const [isContinuing, setIsContinuing] = useState(false)
  const isContinuingRef = useRef(false)
  const timeLabel = formatMessageTime(timestamp, language, t)
  const canCopy = Boolean(content.trim())
  const Icon = copied ? Check : Copy

  useEffect(() => {
    if (!copied) return undefined

    const timerId = window.setTimeout(() => {
      setCopied(false)
    }, COPIED_INDICATOR_MS)

    return () => {
      window.clearTimeout(timerId)
    }
  }, [copied])

  return (
    <div className="chat-message__actions" aria-label={t('chat.messageActions')}>
      {timeLabel && <time dateTime={new Date(timestamp ?? 0).toISOString()}>{timeLabel}</time>}
      <button
        aria-label={copied ? t('chat.copied') : t('chat.copyMessage')}
        disabled={!canCopy}
        onClick={() => {
          if (!canCopy) return
          void copyTextToClipboard(content)
            .then(() => setCopied(true))
            .catch(() => setCopied(false))
        }}
        title={copied ? t('chat.copied') : t('chat.copyMessage')}
        type="button"
      >
        <Icon aria-hidden="true" />
        <span className="chat-message__action-tooltip" role="tooltip">
          {copied ? t('chat.copied') : t('chat.copy')}
        </span>
      </button>
      {canEdit && (
        <button
          aria-label={t('chat.editMessage')}
          onClick={onEdit}
          title={t('chat.editMessage')}
          type="button"
        >
          <Pencil aria-hidden="true" />
          <span className="chat-message__action-tooltip" role="tooltip">
            {t('chat.edit')}
          </span>
        </button>
      )}
      {showTokenUsageDetails && <UsageAction usage={usage} />}
      {onContinueInNewTask && (
        <button
          aria-label={t('chat.continueInNewTask')}
          disabled={isContinuing}
          onClick={() => {
            if (isContinuingRef.current) return
            isContinuingRef.current = true
            setIsContinuing(true)
            void Promise.resolve(onContinueInNewTask()).finally(() => {
              isContinuingRef.current = false
              setIsContinuing(false)
            })
          }}
          title={t('chat.continueInNewTask')}
          type="button"
        >
          {isContinuing ? (
            <LoaderCircle aria-hidden="true" className="chat-message__action-spinner" />
          ) : (
            <Split aria-hidden="true" />
          )}
          <span className="chat-message__action-tooltip" role="tooltip">
            {t('chat.continueInNewTask')}
          </span>
        </button>
      )}
    </div>
  )
}

function AgentThinkingActivity() {
  const { t } = useFrontendConfig()

  return (
    <div className="agent-thinking">
      <span className="agent-running-text">{t('agent.thinking')}</span>
    </div>
  )
}

function AgentTimelineItemView({
  item,
  projectId,
  run
}: {
  item: RenderableTimelineItem
  projectId?: string | null
  run: ChatAgentRunView
}) {
  if (item.type === 'read_group') {
    const items = getReadGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <ReadToolActivityGroup items={items} projectId={projectId} />
  }

  if (item.type === 'search_group') {
    const items = getSearchGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <SearchToolActivityGroup items={items} kind={item.kind} />
  }

  if (item.type === 'web_activity_group') {
    const items = getWebActivityGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <WebSearchToolActivityGroup items={items} />
  }

  if (item.type === 'run_command_group') {
    const items = getRunCommandGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <RunCommandToolActivityGroup items={items} />
  }

  if (item.type === 'apply_patch_group') {
    const items = getApplyPatchGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <ApplyPatchToolActivityGroup items={items} projectId={projectId} />
  }

  if (item.type === 'write_file_group') {
    const items = getWriteFileGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <FileWriteToolActivityGroup items={items} />
  }

  if (item.type === 'conversation_history_group') {
    const items = getConversationHistoryGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <ConversationHistoryToolActivity items={items} />
  }

  if (item.type === 'context_compaction') {
    return <ContextCompactionActivity status={item.status} />
  }

  if (item.type === 'message') {
    if (!item.content.trim()) return null
    return <ChatMarkdown className="chat-agent-text" content={item.content} />
  }

  if (item.type === 'tool_call') {
    const call = run.toolCalls.find((candidate) => candidate.id === item.callId)
    if (!call) return null
    const webActivity = run.webSearchActivities?.find((candidate) => candidate.callId === call.id)
    const readActivity = run.readActivities?.find((candidate) => candidate.callId === call.id)
    const diff =
      call.tool === 'apply_patch'
        ? run.diffs.find((candidate) => candidate.id === call.id)
        : undefined
    const result = getToolResult(run, call.id)
    const previousTodoResult =
      call.tool === 'todo_update' ? getPreviousSuccessfulTodoResult(run, call.id) : undefined
    const settledStatus = getSettledToolStatus(run, result)
    return (
      <AgentToolActivity
        cancelled={settledStatus === 'cancelled'}
        call={call}
        diff={diff}
        projectId={projectId}
        previousTodoResult={previousTodoResult}
        readActivity={readActivity}
        result={result}
        settledStatus={settledStatus}
        webActivity={webActivity}
      />
    )
  }

  return (
    <div className="agent-activity agent-activity--error">
      <AlertTriangle aria-hidden="true" />
      <span>{item.message}</span>
    </div>
  )
}

function AgentRunView({
  message,
  onUiStateChange,
  projectId
}: {
  message: ChatMessage
  onUiStateChange?: (messageId: string, uiState: ChatMessage['uiState']) => void
  projectId?: string | null
}) {
  const { t } = useFrontendConfig()
  const run = message.agentRun
  const timeline = useMemo(() => run?.timeline ?? [], [run?.timeline])
  const runId = run?.runId
  const runIsSettled = !run || isRunSettled(run)
  const hasTimeline = timeline.length > 0
  const hasTimelineError = timeline.some((item) => item.type === 'error')
  const canToggleTimeline = Boolean(
    run && isRunSettled(run) && hasCollapsibleTimelineContent(run, timeline)
  )
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (runIsSettled) return undefined

    const timerId = window.setInterval(() => {
      setNow(Date.now())
    }, 500)

    return () => {
      window.clearInterval(timerId)
    }
  }, [runId, runIsSettled])

  const headerState = useMemo(() => {
    const hasFirstResponse = Boolean(run?.firstResponseAt)
    const hasVisibleToolStatus = Boolean(run && hasCollapsibleTimelineContent(run, timeline))

    if (run && !hasFirstResponse && !hasVisibleToolStatus && !isRunSettled(run)) {
      return {
        isThinking: true,
        label: t('agent.thinking')
      }
    }

    const startedAt = run?.startedAt ?? message.createdAt
    const endedAt = run?.completedAt ?? now
    if (run?.status === 'cancelled') {
      return {
        isThinking: false,
        label: formatTranslation(t, 'agent.stoppedAfter', {
          duration: formatElapsedDuration(endedAt - startedAt)
        })
      }
    }

    return {
      isThinking: false,
      label: formatTranslation(t, 'agent.processed', {
        duration: formatElapsedDuration(endedAt - startedAt)
      })
    }
  }, [message.createdAt, now, run, t, timeline])
  const displayTimeline = useMemo(
    () => (run ? groupTimelineItems(run, timeline) : []),
    [run, timeline]
  )

  if (!run) {
    return hasDisplayableContent(message.content) ? (
      <ChatMarkdown className="chat-agent-text" content={message.content} />
    ) : null
  }

  const timelineCollapsed = canToggleTimeline ? (message.uiState?.timelineCollapsed ?? true) : false
  const showTimeline = hasTimeline && !(canToggleTimeline && timelineCollapsed)
  const finalAnswerContent = getLastMessageTimelineContent(timeline) || message.content
  const showFinalContent =
    hasDisplayableContent(finalAnswerContent) &&
    (!hasTimeline || (canToggleTimeline && timelineCollapsed))
  const isStreamingAssistantText =
    !isRunSettled(run) &&
    Boolean(run.lastResponseAt) &&
    now - (run.lastResponseAt ?? 0) <= ACTIVE_STREAMING_GRACE_MS
  const isStreamingFileWrite = hasRecentFileWriteActivity(run, now)
  const showThinkingActivity =
    !(canToggleTimeline && timelineCollapsed) &&
    !headerState.isThinking &&
    !isStreamingAssistantText &&
    !isStreamingFileWrite &&
    shouldShowThinkingActivity(run, timeline)
  const showTokenLimitNotice = isRunSettled(run) && isTokenLimitFinishReason(run.finishReason)
  const webSearchSources = getUniqueWebSearchSources(run)

  return (
    <div className="agent-run">
      <AgentRunElapsedHeader
        canToggle={canToggleTimeline}
        collapsed={timelineCollapsed}
        isThinking={headerState.isThinking}
        label={headerState.label}
        onToggle={() =>
          onUiStateChange?.(message.id, {
            ...message.uiState,
            timelineCollapsed: !timelineCollapsed
          })
        }
      />
      {showTimeline &&
        displayTimeline.map((item) => (
          <AgentTimelineItemView item={item} key={item.id} projectId={projectId} run={run} />
        ))}
      {showFinalContent && (
        <ChatMarkdown className="chat-agent-text" content={finalAnswerContent} />
      )}
      {isRunSettled(run) && <EditSummaryCard projectId={projectId} run={run} />}
      {isRunSettled(run) && <AssistantSources sources={webSearchSources} />}
      {showTokenLimitNotice && (
        <div className="agent-run__notice" role="status">
          <AlertTriangle aria-hidden="true" />
          <span>{t('agent.tokenLimitNotice')}</span>
        </div>
      )}
      {showThinkingActivity && <AgentThinkingActivity />}
      {showTimeline && run.error && !hasTimelineError && (
        <div className="agent-run__error">{run.error}</div>
      )}
    </div>
  )
}

function MessageContent({ message, onUiStateChange, projectId }: ChatMessageItemProps) {
  if (message.role === 'assistant') {
    return (
      <AgentRunView message={message} onUiStateChange={onUiStateChange} projectId={projectId} />
    )
  }

  return <ChatMarkdown content={getUserVisibleContent(message)} />
}

function EditableUserMessage({
  hasAttachments,
  hasImageAttachments,
  initialContent,
  selectedModelAvailable,
  selectedModelSupportsImage,
  onCancel,
  onSubmit
}: {
  hasAttachments: boolean
  hasImageAttachments: boolean
  initialContent: string
  selectedModelAvailable: boolean
  selectedModelSupportsImage: boolean
  onCancel: () => void
  onSubmit: (content: string) => void | Promise<void>
}) {
  const { t } = useFrontendConfig()
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const [content, setContent] = useState(initialContent)
  const [error, setError] = useState<string | null>(null)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const validationMessage = !selectedModelAvailable
    ? t('chat.noEnabledModels')
    : hasImageAttachments && !selectedModelSupportsImage
      ? t('chat.unsupportedImageWarning')
      : null
  const canSend = (content.trim().length > 0 || hasAttachments) && !validationMessage

  useEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return

    textarea.style.height = 'auto'
    textarea.style.height = `${textarea.scrollHeight}px`
  }, [content])

  const submit = async () => {
    if (!canSend || isSubmitting) return

    setIsSubmitting(true)
    setError(null)
    try {
      await onSubmit(content.trim())
    } catch (submitError) {
      setError(submitError instanceof Error ? submitError.message : String(submitError))
      setIsSubmitting(false)
    }
  }

  return (
    <form
      className="chat-message-edit"
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
    >
      <textarea
        ref={textareaRef}
        aria-label={t('chat.editMessage')}
        autoFocus
        value={content}
        onChange={(event) => setContent(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== 'Enter' || event.shiftKey) return
          event.preventDefault()
          void submit()
        }}
      />
      {validationMessage && <p className="chat-message-edit__warning">{validationMessage}</p>}
      {error && <p className="chat-message-edit__error">{error}</p>}
      <div className="chat-message-edit__actions">
        <button disabled={isSubmitting} onClick={onCancel} type="button">
          {t('chat.cancel')}
        </button>
        <button disabled={!canSend || isSubmitting} type="submit">
          {t('chat.send')}
        </button>
      </div>
    </form>
  )
}

function MessageAttachments({
  attachments,
  messageId
}: {
  attachments?: ChatMessage['attachments']
  messageId: string
}) {
  const { t } = useFrontendConfig()
  const openImagePreview = useImagePreview()
  const showImagePreviewNotice = useImagePreviewNotice()
  if (!attachments?.length) return null

  const imageAttachments = attachments.filter((attachment) => attachment.kind === 'image')
  const fileAttachments = attachments.filter((attachment) => attachment.kind !== 'image')

  const openOriginalImageAttachment = async (
    attachment: NonNullable<ChatMessage['attachments']>[number]
  ) => {
    if (
      attachment.encoding === 'base64' &&
      attachment.mimeType?.startsWith('image/') &&
      attachment.data
    ) {
      openImagePreview({
        alt: attachment.name,
        fileName: attachment.name,
        src: `data:${attachment.mimeType};base64,${attachment.data}`
      })
      return
    }

    try {
      const image = await loadAttachmentImage(attachment.id)
      if (!image?.mimeType.startsWith('image/') || !image.data) {
        showImagePreviewNotice(t('imagePreview.originalMissing'))
        return
      }

      openImagePreview({
        alt: image.name || attachment.name,
        fileName: image.name || attachment.name,
        src: `data:${image.mimeType};base64,${image.data}`
      })
    } catch (error) {
      console.error('Failed to load attachment image', error)
      showImagePreviewNotice(t('imagePreview.originalMissing'))
    }
  }

  const renderAttachment = (attachment: NonNullable<ChatMessage['attachments']>[number]) => {
    const extension = getAttachmentExtension(attachment.name)
    const AttachmentIcon = getAttachmentIcon(attachment.kind, extension)
    const badgeLabel = getAttachmentBadgeLabel(extension)
    const previewUrl = getAttachmentPreviewUrl(attachment)
    const isImagePreview = attachment.kind === 'image' && Boolean(previewUrl)

    if (isImagePreview) {
      return (
        <button
          className="chat-message-attachment"
          data-chat-attachment-id={attachment.id}
          data-chat-attachment-message-id={messageId}
          data-kind="image"
          key={attachment.id}
          onClick={() => void openOriginalImageAttachment(attachment)}
          title={attachment.name}
          type="button"
        >
          <img src={previewUrl} alt={attachment.name} />
        </button>
      )
    }

    return (
      <div
        className="chat-message-attachment"
        data-chat-attachment-id={attachment.id}
        data-chat-attachment-message-id={messageId}
        data-kind="file"
        key={attachment.id}
        title={attachment.name}
      >
        <span className="chat-message-attachment__icon" aria-hidden="true">
          {badgeLabel ? (
            <span className="chat-message-attachment__badge">{badgeLabel}</span>
          ) : (
            <AttachmentIcon />
          )}
        </span>
        <span className="chat-message-attachment__name">{attachment.name}</span>
      </div>
    )
  }

  return (
    <div className="chat-message__attachments" aria-label={t('chat.attachments')}>
      {imageAttachments.length > 0 && (
        <div className="chat-message__attachment-row" data-kind="image">
          {imageAttachments.map(renderAttachment)}
        </div>
      )}
      {fileAttachments.length > 0 && (
        <div className="chat-message__attachment-row" data-kind="file">
          {fileAttachments.map(renderAttachment)}
        </div>
      )}
    </div>
  )
}

export function ChatMessageItem({
  editSelectedModelAvailable = true,
  editSelectedModelSupportsImage = true,
  isLastAssistantMessage = false,
  message,
  onApprove,
  onCancel,
  onEditSubmit,
  onContinueInNewTask,
  onReject,
  onUiStateChange,
  projectId,
  showTokenUsageDetails
}: ChatMessageItemProps) {
  const [isEditing, setIsEditing] = useState(false)
  const isAssistantActionsVisible = shouldShowAssistantActions(message)
  const userVisibleContent = getUserVisibleContent(message)
  const actionContent =
    message.role === 'assistant' ? getAssistantFinalContent(message) : userVisibleContent
  const actionTimestamp =
    message.role === 'assistant'
      ? (message.agentRun?.completedAt ?? message.createdAt)
      : message.createdAt
  const actionUsage = message.role === 'assistant' ? message.agentRun?.usage : undefined
  const showActions = message.role === 'user' || isAssistantActionsVisible
  const canEdit = message.role === 'user' && Boolean(onEditSubmit)
  const pinCopyAction =
    message.role === 'assistant' && isLastAssistantMessage && isAssistantActionsVisible
  const showBody = message.role === 'assistant' || Boolean(userVisibleContent.trim())

  useEffect(() => {
    setIsEditing(false)
  }, [canEdit, message.id])

  return (
    <article
      className={`chat-message chat-message--${message.role}`}
      data-copy-pinned={pinCopyAction ? 'true' : undefined}
      data-editing={isEditing ? 'true' : undefined}
      data-message-id={message.id}
      data-status={message.status}
      key={message.id}
    >
      {message.role === 'user' && (
        <MessageAttachments attachments={message.attachments} messageId={message.id} />
      )}
      {isEditing ? (
        <div className="chat-message__body">
          <EditableUserMessage
            hasAttachments={Boolean(message.attachments?.length)}
            hasImageAttachments={Boolean(
              message.attachments?.some((attachment) => attachment.kind === 'image')
            )}
            initialContent={userVisibleContent}
            onCancel={() => setIsEditing(false)}
            onSubmit={(content) => onEditSubmit?.(message.id, content)}
            selectedModelAvailable={editSelectedModelAvailable}
            selectedModelSupportsImage={editSelectedModelSupportsImage}
          />
        </div>
      ) : showBody ? (
        <div className="chat-message__body">
          <MessageContent
            message={message}
            onApprove={onApprove}
            onCancel={onCancel}
            onReject={onReject}
            onUiStateChange={onUiStateChange}
            projectId={projectId}
            showTokenUsageDetails={showTokenUsageDetails}
          />
        </div>
      ) : null}
      {showActions && !isEditing && (
        <ChatMessageActions
          canEdit={canEdit}
          content={actionContent}
          onEdit={() => setIsEditing(true)}
          onContinueInNewTask={
            message.role === 'assistant' && onContinueInNewTask
              ? () => onContinueInNewTask(message.id)
              : undefined
          }
          showTokenUsageDetails={showTokenUsageDetails}
          timestamp={actionTimestamp}
          usage={actionUsage}
        />
      )}
    </article>
  )
}
