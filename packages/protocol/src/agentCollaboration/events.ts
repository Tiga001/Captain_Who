import type {
  CollaborationTransmission,
  CollaborationEventEnvelope,
  CollaborationActivitySnapshot,
  CollaborationActivitySemantic,
  CollaborationEventKind,
  AgentObserverEventEnvelope,
  CollaborationEventsRequest,
  CollaborationEventsPage,
  CollaborationResyncEnvelope
} from './types'
import {
  record,
  exact,
  text,
  oneOf,
  activitySchema,
  integer,
  eventSchema,
  nullableText,
  schema,
  bool
} from './validation'
import { parseObserverStreamCursor } from './observer'
import { parseAgentEventForHost } from '../agentParsers/events'
import type { AgentEvent } from '../agent'

const MAX_EVENT_PAGE = 512

function parseCollaborationTransmission(value: unknown): CollaborationTransmission {
  const context = 'CollaborationTransmission'
  const item = record(value, context)
  exact(item, ['id', 'kind', 'sourceAgentId', 'targetAgentId'], context)
  const parseId = (value: unknown, field: string): string => {
    const id = text(value, `${context}.${field}`)
    if (id.includes('\0')) throw new Error(`Invalid ${context}.${field}`)
    return id
  }
  return {
    id: parseId(item.id, 'id'),
    kind: oneOf(
      item.kind,
      ['message', 'task', 'user_message', 'completion'] as const,
      `${context}.kind`
    ),
    sourceAgentId:
      item.sourceAgentId === null ? null : parseId(item.sourceAgentId, 'sourceAgentId'),
    targetAgentId: item.targetAgentId === null ? null : parseId(item.targetAgentId, 'targetAgentId')
  }
}

export function parseCollaborationEventEnvelope(value: unknown): CollaborationEventEnvelope {
  const item = record(value, 'CollaborationEventEnvelope')
  exact(
    item,
    [
      'schemaVersion',
      'eventId',
      'sequence',
      'workspaceId',
      'projectId',
      'rootAgentId',
      'rootConversationId',
      'agentId',
      'conversationId',
      'turnId',
      'runId',
      'messageId',
      'kind',
      'resourceRevision',
      'activities',
      'occurredAt',
      ...('transmission' in item ? ['transmission'] : [])
    ],
    'CollaborationEventEnvelope'
  )
  if (!Array.isArray(item.activities))
    throw new Error('Invalid CollaborationEventEnvelope.activities')
  const activityText = (value: unknown, context: string, maximum = 256): string => {
    const parsed = text(value, context, maximum)
    if (parsed.includes('\0')) throw new Error(`Invalid ${context}`)
    return parsed
  }
  const activities = item.activities.map((value) => {
    const snapshot = record(value, 'CollaborationActivitySnapshot')
    exact(
      snapshot,
      [
        'schemaVersion',
        'activityId',
        'semantic',
        'agentId',
        'taskNameSnapshot',
        'ownerAgentId',
        'ownerConversationId',
        'taskMessageId',
        'anchorMessageId',
        'traceBoundarySequence'
      ],
      'CollaborationActivitySnapshot'
    )
    const activity: CollaborationActivitySnapshot = {
      schemaVersion: activitySchema(snapshot.schemaVersion, 'CollaborationActivitySnapshot'),
      activityId: activityText(
        snapshot.activityId,
        'CollaborationActivitySnapshot.activityId',
        2048
      ),
      semantic: oneOf(
        snapshot.semantic,
        ['started', 'updated', 'waiting_approval', 'completed', 'failed', 'interrupted'] as const,
        'CollaborationActivitySnapshot.semantic'
      ),
      agentId: activityText(snapshot.agentId, 'CollaborationActivitySnapshot.agentId'),
      taskNameSnapshot: activityText(
        snapshot.taskNameSnapshot,
        'CollaborationActivitySnapshot.taskNameSnapshot',
        256
      ),
      ownerAgentId: activityText(
        snapshot.ownerAgentId,
        'CollaborationActivitySnapshot.ownerAgentId'
      ),
      ownerConversationId: activityText(
        snapshot.ownerConversationId,
        'CollaborationActivitySnapshot.ownerConversationId'
      ),
      taskMessageId:
        snapshot.taskMessageId === null
          ? null
          : activityText(
              snapshot.taskMessageId,
              'CollaborationActivitySnapshot.taskMessageId',
              2048
            ),
      anchorMessageId:
        snapshot.anchorMessageId === null
          ? null
          : activityText(
              snapshot.anchorMessageId,
              'CollaborationActivitySnapshot.anchorMessageId',
              2048
            ),
      traceBoundarySequence:
        snapshot.traceBoundarySequence === null
          ? null
          : integer(
              snapshot.traceBoundarySequence,
              'CollaborationActivitySnapshot.traceBoundarySequence',
              0
            )
    }
    if (activity.anchorMessageId === null && activity.traceBoundarySequence !== null) {
      throw new Error(
        'Invalid CollaborationActivitySnapshot: trace placement requires an anchor message'
      )
    }
    if ((activity.semantic === 'updated') !== (activity.taskMessageId === null)) {
      throw new Error('Invalid CollaborationActivitySnapshot task identity')
    }
    return activity
  })
  if (new Set(activities.map((activity) => activity.activityId)).size !== activities.length) {
    throw new Error('Invalid CollaborationEventEnvelope duplicate activity identity')
  }
  const parsed: CollaborationEventEnvelope = {
    ...('transmission' in item
      ? { transmission: parseCollaborationTransmission(item.transmission) }
      : {}),
    schemaVersion: eventSchema(item.schemaVersion, 'CollaborationEventEnvelope'),
    eventId: text(item.eventId, 'eventId'),
    sequence: integer(item.sequence, 'sequence', 1),
    workspaceId: nullableText(item.workspaceId, 'workspaceId'),
    projectId: nullableText(item.projectId, 'projectId'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agentId: text(item.agentId, 'agentId'),
    conversationId: text(item.conversationId, 'conversationId'),
    turnId: nullableText(item.turnId, 'turnId'),
    runId: nullableText(item.runId, 'runId'),
    messageId: nullableText(item.messageId, 'messageId'),
    kind: oneOf(
      item.kind,
      [
        'agent_created',
        'agent_updated',
        'mailbox_enqueued',
        'mailbox_updated',
        'wake_created',
        'wake_updated',
        'turn_started',
        'turn_updated',
        'approval_projected',
        'approval_updated'
      ] as const,
      'kind'
    ),
    resourceRevision: integer(item.resourceRevision, 'resourceRevision', 1),
    activities,
    occurredAt: integer(item.occurredAt, 'occurredAt')
  }
  if (parsed.workspaceId !== parsed.projectId) {
    throw new Error('Invalid CollaborationEventEnvelope identity')
  }
  if (parsed.transmission) {
    const { kind, sourceAgentId, targetAgentId } = parsed.transmission
    const isRootEvent =
      parsed.agentId === parsed.rootAgentId && parsed.conversationId === parsed.rootConversationId
    const validTransfer =
      kind === 'user_message'
        ? sourceAgentId === null &&
          targetAgentId === parsed.rootAgentId &&
          isRootEvent &&
          parsed.kind === 'turn_started'
        : kind === 'completion'
          ? sourceAgentId === parsed.rootAgentId &&
            targetAgentId === null &&
            isRootEvent &&
            parsed.kind === 'turn_updated'
          : sourceAgentId !== null &&
            targetAgentId !== null &&
            sourceAgentId !== targetAgentId &&
            targetAgentId === parsed.agentId &&
            parsed.kind === 'mailbox_enqueued'
    if (!validTransfer) {
      throw new Error('Invalid CollaborationEventEnvelope transmission identity')
    }
  }
  for (const activity of parsed.activities) {
    const ownerIsRoot = activity.ownerAgentId === parsed.rootAgentId
    const ownerConversationIsRoot = activity.ownerConversationId === parsed.rootConversationId
    const hasValidOwner =
      activity.ownerAgentId !== activity.agentId &&
      ownerIsRoot === ownerConversationIsRoot &&
      (activity.semantic === 'updated'
        ? activity.ownerAgentId === parsed.agentId &&
          activity.ownerConversationId === parsed.conversationId
        : activity.ownerConversationId !== parsed.conversationId)
    if (!hasValidOwner) {
      throw new Error('Invalid CollaborationEventEnvelope activity owner identity')
    }
    const expectedKind: Readonly<Record<CollaborationActivitySemantic, CollaborationEventKind>> = {
      started: 'wake_created',
      updated: 'mailbox_enqueued',
      waiting_approval: 'approval_projected',
      completed: 'wake_updated',
      failed: 'wake_updated',
      interrupted: 'wake_updated'
    }
    const hasValidSubject =
      activity.semantic === 'updated'
        ? activity.agentId !== parsed.agentId
        : activity.agentId === parsed.agentId
    if (parsed.kind !== expectedKind[activity.semantic] || !hasValidSubject) {
      throw new Error('Invalid CollaborationEventEnvelope activity')
    }
  }
  return parsed
}

export function parseAgentObserverEventEnvelope(value: unknown): AgentObserverEventEnvelope {
  const item = record(value, 'AgentObserverEventEnvelope')
  exact(
    item,
    [
      'schemaVersion',
      'rootAgentId',
      'rootConversationId',
      'agentId',
      'conversationId',
      'runId',
      'assistantMessageId',
      'event',
      ...('streamCursor' in item ? ['streamCursor'] : [])
    ],
    'AgentObserverEventEnvelope'
  )
  const runId = text(item.runId, 'runId')
  const event = parseAgentEventForHost(item.event)
  const boundEvent: AgentEvent =
    event.type === 'error' && event.runId === undefined ? { ...event, runId } : event
  const parsed: AgentObserverEventEnvelope = {
    ...(item.streamCursor === undefined
      ? {}
      : { streamCursor: parseObserverStreamCursor(item.streamCursor) }),
    schemaVersion: schema(item.schemaVersion, 'AgentObserverEventEnvelope'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agentId: text(item.agentId, 'agentId'),
    conversationId: text(item.conversationId, 'conversationId'),
    runId,
    assistantMessageId: text(item.assistantMessageId, 'assistantMessageId'),
    event: boundEvent
  }
  if (parsed.event.runId !== parsed.runId) {
    throw new Error('Invalid AgentObserverEventEnvelope run identity')
  }
  if (
    (parsed.event.type === 'command_started' ||
      parsed.event.type === 'command_output' ||
      parsed.event.type === 'command_exited' ||
      parsed.event.type === 'command_interrupted') &&
    (parsed.event.conversationId !== parsed.conversationId ||
      parsed.event.assistantMessageId !== parsed.assistantMessageId)
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope Command identity')
  }
  if (
    parsed.event.type === 'file_change_updated' &&
    parsed.event.fileChange.conversationId !== parsed.conversationId
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope FileChange identity')
  }
  if (
    parsed.event.type === 'context_window_updated' &&
    parsed.event.conversationId !== undefined &&
    parsed.event.conversationId !== parsed.conversationId
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope context window identity')
  }
  if (
    parsed.event.type === 'state' &&
    parsed.event.state.activeRunId !== null &&
    parsed.event.state.activeRunId !== parsed.runId
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope state identity')
  }
  return parsed
}

export function parseCollaborationEventsRequest(value: unknown): CollaborationEventsRequest {
  const item = record(value, 'CollaborationEventsRequest')
  exact(item, ['rootConversationId', 'afterSequence', 'limit'], 'CollaborationEventsRequest')
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    afterSequence: integer(item.afterSequence, 'afterSequence'),
    limit: integer(item.limit, 'limit', 1, MAX_EVENT_PAGE)
  }
}

export function parseCollaborationEventsPage(value: unknown): CollaborationEventsPage {
  const item = record(value, 'CollaborationEventsPage')
  exact(
    item,
    ['schemaVersion', 'rootAgentId', 'rootConversationId', 'events', 'lastSequence', 'hasMore'],
    'CollaborationEventsPage'
  )
  if (!Array.isArray(item.events) || item.events.length > MAX_EVENT_PAGE)
    throw new Error('Invalid events')
  const parsed = {
    schemaVersion: schema(item.schemaVersion, 'CollaborationEventsPage'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    events: item.events.map(parseCollaborationEventEnvelope),
    lastSequence: integer(item.lastSequence, 'lastSequence'),
    hasMore: bool(item.hasMore, 'hasMore')
  }
  if (
    parsed.events.some(
      (event, index) =>
        event.rootAgentId !== parsed.rootAgentId ||
        event.rootConversationId !== parsed.rootConversationId ||
        (index > 0 && event.sequence !== parsed.events[index - 1]!.sequence + 1)
    ) ||
    (parsed.events.length > 0 && parsed.lastSequence !== parsed.events.at(-1)?.sequence)
  ) {
    throw new Error('Invalid CollaborationEventsPage identity')
  }
  return parsed
}

export function parseCollaborationResyncEnvelope(value: unknown): CollaborationResyncEnvelope {
  const item = record(value, 'CollaborationResyncEnvelope')
  exact(item, ['schemaVersion', 'reason'], 'CollaborationResyncEnvelope')
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationResyncEnvelope'),
    reason: oneOf(item.reason, ['core_started', 'model_settings_changed'] as const, 'reason')
  }
}
