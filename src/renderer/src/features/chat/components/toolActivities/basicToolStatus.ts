import type { AgentFileChangeOperation, AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { ChatAgentRunView } from '../../chatTypes'
import {
  getBasicToolRepresentative,
  type BasicToolCategory,
  type BasicToolItem
} from '../basicToolTimeline'
import {
  getFileChangeGroupItems,
  getSettledToolStatus,
  isRunSettled
} from '../chatMessageItemUtils'
import { getFileChangeItemView } from './FileChangeToolActivity'
import { getRunCommandStatus } from './RunCommandToolActivity'

export type BasicToolPhase =
  | 'awaiting_approval'
  | 'preparing'
  | 'ready'
  | 'starting'
  | 'running'
  | 'completed'
  | 'failed'
  | 'conflict'
  | 'rejected'
  | 'cancelled'
  | 'interrupted'
  | 'timed_out'
  | 'outcome_unknown'
export type BasicToolAttention = 'failed' | 'unknown' | 'rejected' | 'cancelled'
export type BasicToolSummaryCategory = BasicToolCategory | 'create' | 'delete'

export interface BasicToolItemStatus {
  item: BasicToolItem
  call: AgentToolCall
  result?: AgentToolResult
  phase: BasicToolPhase
  subject: string
  isActive: boolean
  isPending: boolean
  attention?: BasicToolAttention
  editOperation?: AgentFileChangeOperation
  editCounts?: { additions: number; deletions: number }
}

export interface BasicToolGroupPresentation {
  items: BasicToolItemStatus[]
  selected?: BasicToolItemStatus
  categoryCounts: Partial<Record<BasicToolSummaryCategory, number>>
  /** Unique outcomes, without success/failure counters. */
  outcomes: BasicToolAttention[]
  /** An earlier settled issue remains visible beside the current action. */
  attention?: BasicToolAttention
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function subjectFor(call: AgentToolCall, run: ChatAgentRunView): string {
  const args = record(call.args)
  if (call.tool === 'run_command') return text(args.reason) || text(call.reason)
  if (call.tool === 'search_files' || call.tool === 'search_code') return text(args.query)
  if (call.tool === 'workspace_map') return text(args.focusPath)
  if (call.tool.startsWith('read_')) {
    return (
      run.readActivities?.find((activity) => activity.callId === call.id)?.path ||
      text(args.path) ||
      text(args.filePath)
    )
  }
  return ''
}

function attentionFor(phase: BasicToolPhase): BasicToolAttention | undefined {
  if (phase === 'outcome_unknown') return 'unknown'
  if (phase === 'failed' || phase === 'conflict' || phase === 'timed_out') return 'failed'
  if (phase === 'rejected') return 'rejected'
  if (phase === 'cancelled' || phase === 'interrupted') return 'cancelled'
  return undefined
}

/** Uses each tool's authoritative lifecycle, not result presence as a universal completion test. */
export function deriveBasicToolItemStatus(
  run: ChatAgentRunView,
  item: BasicToolItem
): BasicToolItemStatus | undefined {
  const representative = getBasicToolRepresentative(run, item)
  if (!representative) return undefined
  const { call, result } = representative
  const settledStatus = getSettledToolStatus(run, result)
  let phase: BasicToolPhase
  let subject = subjectFor(call, run)
  let editOperation: AgentFileChangeOperation | undefined
  let editCounts: BasicToolItemStatus['editCounts']

  if (item.category === 'command') {
    const session = run.commandSessions?.[call.id]
    const commandStatus = getRunCommandStatus({
      call,
      result,
      session,
      settledStatus,
      cancelled: settledStatus === 'cancelled'
    })
    phase =
      session?.status === 'outcome_unknown' ||
      (!session && record(result?.result).status === 'outcome_unknown')
        ? 'outcome_unknown'
        : commandStatus === 'waiting_for_approval'
          ? 'awaiting_approval'
          : commandStatus
    // A retired approval is no longer actionable. Independently handed-off sessions/receipts
    // retain their own running status even when the parent Run has settled.
    if (phase === 'awaiting_approval' && isRunSettled(run)) phase = settledStatus ?? 'cancelled'
  } else if (item.category === 'edit') {
    const edit = getFileChangeGroupItems(run, item.callIds)[0]
    if (!edit) return undefined
    const view = getFileChangeItemView(edit)
    subject = view.filePath
    editOperation = view.operation
    editCounts = { additions: view.additions, deletions: view.deletions }
    phase =
      view.status === 'applied'
        ? 'completed'
        : view.status === 'waiting'
          ? 'awaiting_approval'
          : view.status
    if (phase === 'running') {
      if (edit.transaction?.status === 'ready') phase = 'ready'
      else if (
        edit.transaction?.status !== 'applying' &&
        (edit.transaction?.status === 'drafting' || edit.preview)
      )
        phase = 'preparing'
    }
    if (phase === 'awaiting_approval' && isRunSettled(run)) phase = settledStatus ?? 'cancelled'
  } else {
    const activity =
      item.category === 'read'
        ? run.readActivities?.find((candidate) => candidate.callId === call.id)
        : undefined
    phase = result
      ? result.ok
        ? 'completed'
        : 'failed'
      : activity && activity.status !== 'running'
        ? activity.status
        : (settledStatus ??
          (call.approvalStatus === 'rejected'
            ? 'rejected'
            : call.approvalStatus === 'required'
              ? 'awaiting_approval'
              : 'running'))
  }

  const isActive = phase === 'running' || phase === 'preparing' || phase === 'starting'
  return {
    item,
    call,
    result,
    phase,
    subject,
    isActive,
    isPending: isActive || phase === 'ready' || phase === 'awaiting_approval',
    attention: attentionFor(phase),
    editOperation,
    editCounts
  }
}

function priority(item: BasicToolItemStatus): number {
  if (item.phase === 'awaiting_approval') return 3
  if (item.isActive) return 2
  if (item.isPending) return 1
  return 0
}

/** Selection never reorders leaves; ordinals belong to the original timeline segment. */
export function getBasicToolGroupPresentation(
  run: ChatAgentRunView,
  items: readonly BasicToolItem[]
): BasicToolGroupPresentation {
  const statuses = items.flatMap((item) => {
    const status = deriveBasicToolItemStatus(run, item)
    return status ? [status] : []
  })
  const categoryCounts: BasicToolGroupPresentation['categoryCounts'] = {}
  let selected: BasicToolItemStatus | undefined
  for (const status of statuses) {
    const category =
      status.editOperation === 'create' || status.editOperation === 'delete'
        ? status.editOperation
        : status.item.category
    categoryCounts[category] = (categoryCounts[category] ?? 0) + 1
    if (
      priority(status) > 0 &&
      (!selected ||
        priority(status) > priority(selected) ||
        (priority(status) === priority(selected) &&
          status.item.latestOrdinal > selected.item.latestOrdinal))
    ) {
      selected = status
    }
  }
  const outcomes = (['unknown', 'failed', 'rejected', 'cancelled'] as const).filter((outcome) =>
    statuses.some((status) => status.attention === outcome)
  )
  return {
    items: statuses,
    selected,
    categoryCounts,
    outcomes,
    attention: selected ? outcomes.find((outcome) => outcome !== 'cancelled') : undefined
  }
}
