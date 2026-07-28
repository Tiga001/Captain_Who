import type { AgentToolCall, AgentToolResult, AgentUsage } from '@mycopilot/protocol'
import type { Translate } from '../../../config/translationFormat'
import { getReadActivityKindForTool, isReadActivityTool } from '../agentReadActivities'
import { stripAttachmentSummary } from '../chatAttachments'
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatFileWritePreview,
  ChatMessage,
  ChatReadActivityKind
} from '../chatTypes'
import type { ApplyPatchToolActivityGroupItem } from './toolActivities/ApplyPatchToolActivity'
import type { ConversationHistoryActivityItem } from './toolActivities/ConversationHistoryToolActivity'
import type { FileWriteToolActivityGroupItem } from './toolActivities/FileWriteToolActivity'
import type { ReadToolActivityGroupItem } from './toolActivities/ReadToolActivity'
import type { RunCommandToolActivityGroupItem } from './toolActivities/RunCommandToolActivity'
import type { OfficeToolActivityGroupItem } from './toolActivities/OfficeToolActivity'
import {
  getSearchKind,
  isSearchTool,
  type SearchKind,
  type SearchToolActivityGroupItem
} from './toolActivities/SearchToolActivity'
import type { SettledToolStatus } from './toolActivities/toolActivityUtils'
import type { WebSearchToolActivityGroupItem } from './toolActivities/WebSearchToolActivity'
import {
  getActivatedSkills,
  getOfficeActivityGroupIdentity,
  getSkillResourceActivityItem,
  isHiddenSkillTool,
  isOfficeTool,
  type SkillResourceActivityItem,
  type SkillResourceActivityKind
} from '../skillOfficeActivity'

const FILE_WRITE_ACTIVITY_GRACE_MS = 2000

export type RenderableTimelineItem =
  | ChatAgentTimelineItem
  | {
      id: string
      type: 'skill_load_group'
    }
  | {
      id: string
      type: 'skill_resource_group'
      kind: SkillResourceActivityKind
      skillId?: string
      callIds: string[]
    }
  | {
      id: string
      type: 'office_group'
      groupKey: string
      callIds: string[]
    }
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

export function formatMessageTime(timestamp: number | undefined, language: string, t: Translate) {
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

export function formatUsageTokenCount(value: unknown, language: string) {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  return new Intl.NumberFormat(language).format(value)
}

export function getUsageRows(usage: AgentUsage | undefined, language: string, t: Translate) {
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

export async function copyTextToClipboard(content: string) {
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

export function getToolResult(run: ChatAgentRunView, callId: string): AgentToolResult | undefined {
  return run.toolResults.find((result) => result.callId === callId)
}

export function getPreviousSuccessfulTodoResult(
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

export function isRunSettled(run: ChatAgentRunView) {
  return (
    run.status === 'completed' ||
    run.status === 'failed' ||
    run.status === 'cancelled' ||
    (run.status === 'idle' && Boolean(run.completedAt))
  )
}

export function getSettledToolStatus(
  run: ChatAgentRunView,
  result: ReturnType<typeof getToolResult>
): SettledToolStatus | undefined {
  if (result || !isRunSettled(run)) return undefined
  if (run.status === 'failed') return 'failed'
  if (run.status === 'cancelled') return 'cancelled'
  if (run.status === 'completed' || (run.status === 'idle' && run.completedAt)) return 'completed'
  return undefined
}

export function isTokenLimitFinishReason(finishReason: string | undefined) {
  if (!finishReason) return false
  const normalized = finishReason.toLowerCase()
  return (
    normalized === 'length' ||
    normalized === 'max_tokens' ||
    normalized === 'max_output_tokens' ||
    normalized.includes('max_token')
  )
}

export function isThinkingPlaceholder(content: string) {
  return content.trim() === '正在思考...'
}

export function hasDisplayableContent(content: string) {
  return Boolean(content.trim()) && !isThinkingPlaceholder(content)
}

export function isTimelineItemRenderable(run: ChatAgentRunView, item: ChatAgentTimelineItem) {
  if (item.type === 'message') return Boolean(item.content.trim())
  if (item.type === 'tool_call') {
    const call = run.toolCalls.find((candidate) => candidate.id === item.callId)
    return Boolean(call && !isHiddenSkillTool(call.tool))
  }
  return true
}

export function getLastRenderableTimelineItem(
  run: ChatAgentRunView,
  timeline: ChatAgentTimelineItem[]
) {
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const item = timeline[index]
    if (isTimelineItemRenderable(run, item)) {
      return item
    }
  }

  return undefined
}

export function getLastMessageTimelineContent(timeline: ChatAgentTimelineItem[]) {
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const item = timeline[index]
    if (item.type === 'message' && hasDisplayableContent(item.content)) {
      return item.content
    }
  }

  return ''
}

export function getAssistantFinalContent(message: ChatMessage) {
  const timelineContent = getLastMessageTimelineContent(message.agentRun?.timeline ?? [])
  // The durable assistant message is the canonical final answer. Timeline messages are execution
  // narration and may be rebuilt from Trace after a reload, where the terminal answer is
  // intentionally not duplicated. Keep the timeline fallback only for legacy/in-progress records
  // whose message content was never finalized.
  return hasDisplayableContent(message.content) ? message.content : timelineContent
}

export function getUserVisibleContent(message: ChatMessage) {
  if (message.role !== 'user' || !message.attachments?.length) return message.content
  return stripAttachmentSummary(message.content, message.attachments)
}

export function isBottomTimelineItemSpecificPendingStatus(
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

export function shouldShowThinkingActivity(
  run: ChatAgentRunView,
  timeline: ChatAgentTimelineItem[]
) {
  if (isRunSettled(run)) return false

  const lastItem = getLastRenderableTimelineItem(run, timeline)
  return !isBottomTimelineItemSpecificPendingStatus(run, lastItem)
}

export function hasRecentFileWriteActivity(run: ChatAgentRunView, now: number): boolean {
  if (isRunSettled(run)) return false
  return Boolean(
    run.fileWritePreviews?.some(
      (preview) => now - preview.receivedAt <= FILE_WRITE_ACTIVITY_GRACE_MS
    )
  )
}

export function hasCollapsibleTimelineContent(
  run: ChatAgentRunView,
  timeline: ChatAgentTimelineItem[]
) {
  return (
    getActivatedSkills(run).length > 0 ||
    timeline.some((item) => item.type !== 'message' && isTimelineItemRenderable(run, item))
  )
}

export function getReadKindForCall(run: ChatAgentRunView, call: AgentToolCall) {
  if (!isReadActivityTool(call.tool)) return undefined
  return (
    run.readActivities?.find((activity) => activity.callId === call.id)?.kind ??
    getReadActivityKindForTool(call.tool)
  )
}

export function groupTimelineItems(
  run: ChatAgentRunView,
  timeline: ChatAgentTimelineItem[]
): RenderableTimelineItem[] {
  const activatedSkills = getActivatedSkills(run)
  const initialItems: RenderableTimelineItem[] = activatedSkills.length
    ? [{ id: `skill-load-${run.runId ?? 'pending'}`, type: 'skill_load_group' }]
    : []

  return timeline.reduce<RenderableTimelineItem[]>((items, item) => {
    // Whitespace-only stream messages are invisible in the timeline, so they must not split
    // otherwise adjacent tool activity groups across model turns.
    if (item.type === 'message' && !item.content.trim()) return items
    if (item.type !== 'tool_call') return [...items, item]

    const call = run.toolCalls.find((candidate) => candidate.id === item.callId)
    if (!call) return [...items, item]

    if (isHiddenSkillTool(call.tool)) return items

    if (isOfficeTool(call.tool)) {
      const identity = getOfficeActivityGroupIdentity(call)
      if (!identity) return [...items, item]
      const previousItem = items[items.length - 1]
      if (previousItem?.type === 'office_group' && previousItem.groupKey === identity.key) {
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
          id: `office-group-${item.callId}`,
          type: 'office_group',
          groupKey: identity.key,
          callIds: [item.callId]
        }
      ]
    }

    if (call.tool === 'skills_read_resource' || call.tool === 'skills_materialize_resource') {
      const resource = getSkillResourceActivityItem(run, call)
      if (!resource) return items
      const previousItem = items[items.length - 1]
      if (
        previousItem?.type === 'skill_resource_group' &&
        previousItem.kind === resource.kind &&
        previousItem.skillId === resource.skill?.id
      ) {
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
          id: `skill-resource-group-${item.callId}`,
          type: 'skill_resource_group',
          kind: resource.kind,
          skillId: resource.skill?.id,
          callIds: [item.callId]
        }
      ]
    }

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
  }, initialItems)
}

export function getSkillResourceGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): SkillResourceActivityItem[] {
  const byResource = new Map<string, SkillResourceActivityItem>()

  callIds.forEach((callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return
    const result = getToolResult(run, call.id)
    const item = getSkillResourceActivityItem(run, call, getSettledToolStatus(run, result))
    if (!item) return

    // Progressive reads of one URI are one user-visible resource. Keep the latest page status
    // while preserving the original insertion order of the resource in this activity group.
    byResource.set(item.resourceKey, item)
  })

  return [...byResource.values()]
}

export function getOfficeGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): OfficeToolActivityGroupItem[] {
  return callIds.flatMap((callId) => {
    const call = run.toolCalls.find((candidate) => candidate.id === callId)
    if (!call) return []
    const result = getToolResult(run, call.id)
    return [{ call, settledStatus: getSettledToolStatus(run, result) }]
  })
}

export function getWriteFileDraftId(
  run: ChatAgentRunView,
  call: AgentToolCall
): string | undefined {
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

export function getLatestWriteFilePreview(
  run: ChatAgentRunView,
  draftId: string | undefined
): ChatFileWritePreview | undefined {
  if (!draftId) return undefined
  return run.fileWritePreviews?.reduce<ChatFileWritePreview | undefined>((latest, preview) => {
    if (preview.draftId !== draftId) return latest
    return !latest || preview.receivedAt >= latest.receivedAt ? preview : latest
  }, undefined)
}

export function getWriteFileGroupItems(
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

export function getReadGroupItems(
  run: ChatAgentRunView,
  callIds: string[]
): ReadToolActivityGroupItem[] {
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

export function getSearchGroupItems(
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

export function getWebActivityGroupItems(
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

export function getRunCommandGroupItems(
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
        liveOutput: run.commandOutputPreviews?.[call.id],
        result,
        settledStatus
      }
    ]
  }, [])
}

export function getApplyPatchGroupItems(
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

export function getConversationHistoryGroupItems(
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

export function shouldShowAssistantActions(message: ChatMessage) {
  if (message.role !== 'assistant' || message.status !== 'sent') return false
  if (!getAssistantFinalContent(message).trim()) return false
  return (
    !message.agentRun ||
    message.agentRun.status === 'completed' ||
    message.agentRun.status === 'idle' ||
    message.agentRun.status === 'cancelled'
  )
}

export function formatElapsedDuration(milliseconds: number) {
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
