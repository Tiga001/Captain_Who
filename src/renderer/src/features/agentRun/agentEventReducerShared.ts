import type { ChatAgentRunView, ChatAgentTimelineItem } from '../chat/chatTypes'

function createAgentRun(
  runId: string | null,
  status: ChatAgentRunView['status'] = 'starting'
): ChatAgentRunView {
  const now = Date.now()

  return {
    runId,
    status,
    startedAt: now,
    completedAt: isCompletedAgentRunStatus(status) ? now : undefined,
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    fileChangeProposals: [],
    fileChanges: [],
    fileChangePreviews: [],
    messageStreamCheckpoints: {},
    webSearchActivities: [],
    readActivities: [],
    mcpInvocations: [],
    timeline: []
  }
}

export function ensureAgentRun(
  currentRun: ChatAgentRunView | undefined,
  runId: string | null | undefined,
  status?: ChatAgentRunView['status']
): ChatAgentRunView {
  if (!currentRun) {
    return createAgentRun(runId ?? null, status)
  }

  return {
    ...currentRun,
    runId: currentRun.runId ?? runId ?? null,
    status: status ?? currentRun.status,
    startedAt: currentRun.startedAt ?? Date.now(),
    completedAt: isCompletedAgentRunStatus(status)
      ? (currentRun.completedAt ?? Date.now())
      : currentRun.completedAt,
    toolDefinitions: Array.isArray(currentRun.toolDefinitions) ? currentRun.toolDefinitions : [],
    toolCalls: Array.isArray(currentRun.toolCalls) ? currentRun.toolCalls : [],
    toolResults: Array.isArray(currentRun.toolResults) ? currentRun.toolResults : [],
    approvals: Array.isArray(currentRun.approvals) ? currentRun.approvals : [],
    ...(Array.isArray(currentRun.skillInstallations)
      ? { skillInstallations: currentRun.skillInstallations }
      : {}),
    fileChangeProposals: Array.isArray(currentRun.fileChangeProposals)
      ? currentRun.fileChangeProposals
      : [],
    timeline: Array.isArray(currentRun.timeline) ? currentRun.timeline : [],
    readActivities: Array.isArray(currentRun.readActivities) ? currentRun.readActivities : [],
    fileChanges: Array.isArray(currentRun.fileChanges) ? currentRun.fileChanges : [],
    fileChangePreviews: Array.isArray(currentRun.fileChangePreviews)
      ? currentRun.fileChangePreviews
      : [],
    messageStreamCheckpoints:
      currentRun.messageStreamCheckpoints &&
      typeof currentRun.messageStreamCheckpoints === 'object' &&
      !Array.isArray(currentRun.messageStreamCheckpoints)
        ? currentRun.messageStreamCheckpoints
        : {}
  }
}

export function isCompletedAgentRunStatus(status: ChatAgentRunView['status'] | undefined) {
  return status === 'completed' || status === 'failed' || status === 'cancelled'
}

export function isFinishedAgentOutputStatus(status: ChatAgentRunView['status'] | undefined) {
  return status !== 'starting' && status !== 'running' && status !== 'waiting_for_approval'
}

export function getSettledActivityStatus(
  status: ChatAgentRunView['status'] | undefined
): 'completed' | 'failed' | 'cancelled' | null {
  if (status === 'completed') return 'completed'
  if (status === 'failed') return 'failed'
  if (status === 'cancelled') return 'cancelled'
  return null
}

export function upsertById<T>(items: T[], nextItem: T, getId: (item: T) => string) {
  const nextId = getId(nextItem)
  const itemIndex = items.findIndex((item) => getId(item) === nextId)

  if (itemIndex === -1) {
    return [...items, nextItem]
  }

  return items.map((item, index) => (index === itemIndex ? nextItem : item))
}

export function appendTimelineItem(
  run: ChatAgentRunView,
  item: ChatAgentTimelineItem
): ChatAgentTimelineItem[] {
  if (run.timeline.some((timelineItem) => timelineItem.id === item.id)) {
    return run.timeline.map((timelineItem) => (timelineItem.id === item.id ? item : timelineItem))
  }

  return [...run.timeline, item]
}
