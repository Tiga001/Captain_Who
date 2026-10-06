import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { ChatAgentRunView, ChatAgentTimelineItem } from '../chatTypes'
import {
  getFileChangeTransactionId,
  groupTimelineItems,
  isHiddenTimelineTool,
  type RenderableTimelineItem
} from './chatMessageItemUtils'

export type BasicToolCategory =
  'read' | 'search' | 'workspace' | 'attachments' | 'history' | 'edit' | 'command'

export interface BasicToolItem {
  /** First raw call marker, independent of the current staged-edit representative. */
  id: string
  callIds: string[]
  /** Raw positions within the supplied timeline segment; never completion/update order. */
  firstOrdinal: number
  latestOrdinal: number
  category: BasicToolCategory
}

export type BasicToolTimelineBlock =
  | { kind: 'basic_tools'; id: string; items: BasicToolItem[] }
  | { kind: 'legacy'; items: RenderableTimelineItem[] }

const BASIC_TOOL_CATEGORIES = new Map<string, BasicToolCategory>([
  ['read_file', 'read'],
  ['read_word', 'read'],
  ['read_presentation', 'read'],
  ['read_spreadsheet', 'read'],
  ['workspace_map', 'workspace'],
  ['search_files', 'search'],
  ['search_code', 'search'],
  ['attachments_list', 'attachments'],
  ['attachments_list_project', 'attachments'],
  ['conversation_history', 'history'],
  ['apply_patch', 'edit'],
  ['run_command', 'command']
])

// These Host extensions own existing hidden/grouped presentation routes. Keep exact ownership
// checks so an unrelated extension borrowing a local Tool name cannot inherit those routes.
const LEGACY_EXTENSION_OWNERS = new Map<string, string>([
  ['skills_activate', 'skills'],
  ['web_search', 'web.search'],
  ['web_fetch', 'web.search']
])

/**
 * Presentation-only spans. Callers split collaboration/final-answer boundaries before projecting.
 * Project raw markers first: legacy file grouping may move later edits into an earlier group.
 */
export function projectBasicToolTimeline(
  run: ChatAgentRunView,
  timeline: ChatAgentTimelineItem[]
): BasicToolTimelineBlock[] {
  const blocks: BasicToolTimelineBlock[] = []
  const callsById = new Map(run.toolCalls.map((call) => [call.id, call]))
  const mcpCallIds = new Set(run.mcpInvocations?.map((invocation) => invocation.callId))
  let basicItems: BasicToolItem[] = []
  let legacyItems: ChatAgentTimelineItem[] = []
  const editsByTransaction = new Map<string, BasicToolItem>()

  const flushBasic = () => {
    if (basicItems.length > 0) {
      blocks.push({
        kind: 'basic_tools',
        id: `basic-tools-${basicItems[0].id}`,
        items: basicItems
      })
    }
    basicItems = []
    editsByTransaction.clear()
  }
  const flushLegacy = () => {
    if (legacyItems.length > 0) {
      const items = groupTimelineItems(run, legacyItems, { includeSkillLoadGroup: false })
      if (items.length > 0) blocks.push({ kind: 'legacy', items })
    }
    legacyItems = []
  }

  timeline.forEach((marker, ordinal) => {
    if (marker.type === 'message' && !marker.content.trim()) return
    const call = marker.type === 'tool_call' ? callsById.get(marker.callId) : undefined
    const trustedLegacyExtension =
      call &&
      marker.type === 'tool_call' &&
      marker.identity?.type === 'runtime_extension' &&
      marker.identity.toolName === call.tool &&
      LEGACY_EXTENSION_OWNERS.get(call.tool) === marker.identity.extensionId
    if (
      marker.type === 'tool_call' &&
      (mcpCallIds.has(marker.callId) ||
        (marker.identity !== undefined &&
          !trustedLegacyExtension &&
          (marker.identity.type !== 'builtin' || marker.identity.toolName !== call?.tool)))
    ) {
      // Foreign provenance wins over local-looking names, including hidden bookkeeping names.
      // Keep the marker identity intact instead of passing it through legacy name-based grouping.
      flushBasic()
      flushLegacy()
      blocks.push({ kind: 'legacy', items: [marker] })
      return
    }
    if (call && isHiddenTimelineTool(call.tool)) return

    const category =
      call &&
      marker.type === 'tool_call' &&
      marker.identity?.type === 'builtin' &&
      marker.identity.toolName === call.tool
        ? BASIC_TOOL_CATEGORIES.get(call.tool)
        : undefined

    if (!category || !call) {
      flushBasic()
      legacyItems.push(marker)
      return
    }

    flushLegacy()
    const transactionId = category === 'edit' ? getFileChangeTransactionId(run, call) : undefined
    const existing = transactionId ? editsByTransaction.get(transactionId) : undefined
    if (existing) {
      existing.callIds.push(call.id)
      existing.latestOrdinal = ordinal
      return
    }

    const item: BasicToolItem = {
      id: marker.id,
      callIds: [call.id],
      firstOrdinal: ordinal,
      latestOrdinal: ordinal,
      category
    }
    basicItems.push(item)
    if (transactionId) editsByTransaction.set(transactionId, item)
  })

  flushBasic()
  flushLegacy()
  return blocks
}

/** Edit views still use getFileChangeGroupItems to retain transaction/result fallback semantics. */
export function getBasicToolRepresentative(
  run: ChatAgentRunView,
  item: BasicToolItem
): { call: AgentToolCall; result: AgentToolResult | undefined } | undefined {
  for (let index = item.callIds.length - 1; index >= 0; index -= 1) {
    const call = run.toolCalls.find((candidate) => candidate.id === item.callIds[index])
    if (call) {
      return { call, result: run.toolResults.find((result) => result.callId === call.id) }
    }
  }
  return undefined
}
