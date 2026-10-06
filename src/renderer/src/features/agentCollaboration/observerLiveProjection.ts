import type {
  AgentEvent,
  AgentObserverEventEnvelope,
  AgentObserverLiveStreamSnapshot,
  AgentObserverStreamCursor
} from '@mycopilot/protocol'
import { applyAgentEventToChatMessage } from '../agentRun/agentEventReducer'
import type { ChatConversation, ChatMessage } from '../chat/chatTypes'

export interface ObserverScope {
  agentId: string
  rootAgentId: string
  rootConversationId: string
  conversationId: string
}

export interface ObserverStreamPosition {
  runId: string
  assistantMessageId: string
  cursor: AgentObserverStreamCursor
}

export function isObserverTransientEvent(event: AgentEvent): boolean {
  return (
    event.type === 'message_delta' ||
    event.type === 'message' ||
    event.type === 'message_stream_started' ||
    event.type === 'message_stream_reset' ||
    event.type === 'message_stream_committed' ||
    event.type === 'model_activity_changed' ||
    event.type === 'final_answer_ready' ||
    event.type === 'llm_retry' ||
    event.type === 'tool_input_progress'
  )
}

/** Restores content-free activity and replaces only the provisional text prefix. */
export function applyObserverStreamSnapshot(
  conversation: ChatConversation,
  snapshot: AgentObserverLiveStreamSnapshot | undefined
): ChatConversation {
  if (!snapshot) return conversation
  const { stream, runId, assistantMessageId, finalAnswerReady } = snapshot
  const modelActivity = finalAnswerReady ? undefined : snapshot.modelActivity
  return {
    ...conversation,
    messages: conversation.messages.map((message) => {
      const run = message.agentRun
      if (
        message.id !== assistantMessageId ||
        run?.runId !== runId ||
        run.status === 'completed' ||
        run.status === 'failed' ||
        run.status === 'cancelled'
      )
        return message
      let next: ChatMessage = message
      if (stream) {
        // A narration may already have committed in SQLite just before its stream-commit event.
        // The start boundary identifies that same narration without guessing from matching text.
        const isCurrentStream = (item: (typeof run.timeline)[number]) =>
          item.type === 'message' &&
          (item.streamId === stream.streamId ||
            (item.traceSequence !== undefined &&
              item.traceSequence >= stream.traceBoundarySequence))
        const insertionIndex = run.timeline.findIndex(isCurrentStream)
        const checkpoints = { ...run.messageStreamCheckpoints }
        delete checkpoints[stream.streamId]
        const attempts = { ...run.modelActivityAttempts }
        delete attempts[stream.streamId]
        next = {
          ...message,
          agentRun: {
            ...run,
            timeline: run.timeline.filter((item) => !isCurrentStream(item)),
            modelActivity: undefined,
            modelActivityAttempts: attempts,
            messageStreamCheckpoints: checkpoints
          }
        }
        next = applyAgentEventToChatMessage(next, {
          type: 'message_stream_started',
          runId,
          streamId: stream.streamId,
          attempt: stream.attempt
        })
        next = applyAgentEventToChatMessage(next, {
          type: 'message_delta',
          runId,
          streamId: stream.streamId,
          delta: stream.content
        })
        if (stream.committed) {
          next = applyAgentEventToChatMessage(next, {
            type: 'message_stream_committed',
            runId,
            streamId: stream.streamId,
            traceSequence: null
          })
        }
        if (insertionIndex >= 0 && next.agentRun) {
          const timeline = [...next.agentRun.timeline]
          const [text] = timeline.splice(timeline.length - 1, 1)
          if (text) timeline.splice(insertionIndex, 0, text)
          next = { ...next, agentRun: { ...next.agentRun, timeline } }
        }
      }
      next = {
        ...next,
        ...(modelActivity || finalAnswerReady ? { status: 'pending' as const } : {}),
        agentRun: {
          ...next.agentRun!,
          ...(modelActivity || finalAnswerReady ? { status: 'running' as const } : {}),
          finalAnswerReady,
          llmRetry: finalAnswerReady ? undefined : next.agentRun?.llmRetry,
          modelActivity,
          modelActivityAttempts: {
            ...next.agentRun?.modelActivityAttempts,
            ...(modelActivity ? { [modelActivity.streamId]: modelActivity.attempt } : {})
          }
        }
      }
      return next
    })
  }
}

/** Transient state uses the snapshot cut; durable events still reduce their own identities. */
export function applyObserverEnvelopeWithCursor(
  conversation: ChatConversation,
  scope: ObserverScope,
  envelope: AgentObserverEventEnvelope,
  position: ObserverStreamPosition | null
): { conversation: ChatConversation; position: ObserverStreamPosition | null } {
  if (!matchesObserverEnvelope(conversation, scope, envelope)) return { conversation, position }
  const cursor = envelope.streamCursor
  const alreadyObserved =
    cursor &&
    position?.runId === envelope.runId &&
    position.assistantMessageId === envelope.assistantMessageId &&
    position.cursor.generation === cursor.generation &&
    position.cursor.sequence >= cursor.sequence
  if (alreadyObserved && isObserverTransientEvent(envelope.event)) {
    return { conversation, position }
  }
  let next = applyObserverLiveEnvelope(conversation, scope, envelope)
  if (alreadyObserved && next !== conversation) {
    // Earlier durable tool/state events may still add timeline rows. Newer activity or a final
    // answer marker owns this presentation phase; terminal status still takes precedence.
    const previous = conversation.messages.find(
      (message) => message.id === envelope.assistantMessageId
    )?.agentRun
    next = {
      ...next,
      messages: next.messages.map((message) => {
        if (message.id !== envelope.assistantMessageId || !message.agentRun) return message
        const terminal = ['completed', 'failed', 'cancelled'].includes(message.agentRun.status)
        const newerPhase =
          previous?.finalAnswerReady ||
          (previous?.modelActivity &&
            (previous.status === 'running' || previous.status === 'starting'))
        if (
          terminal ||
          (!newerPhase &&
            message.agentRun.status !== 'running' &&
            message.agentRun.status !== 'starting')
        )
          return message
        return {
          ...message,
          ...(newerPhase
            ? { status: conversation.messages.find((item) => item.id === message.id)?.status }
            : {}),
          agentRun: {
            ...message.agentRun,
            ...(newerPhase ? { status: previous.status } : {}),
            finalAnswerReady: previous?.finalAnswerReady,
            modelActivity: previous?.modelActivity,
            modelActivityAttempts: previous?.modelActivityAttempts
          }
        }
      })
    }
  }
  return {
    conversation: next,
    position:
      cursor && !alreadyObserved
        ? { runId: envelope.runId, assistantMessageId: envelope.assistantMessageId, cursor }
        : position
  }
}

function matchesObserverEnvelope(
  conversation: ChatConversation,
  scope: ObserverScope,
  envelope: AgentObserverEventEnvelope
): boolean {
  return (
    envelope.agentId === scope.agentId &&
    envelope.rootAgentId === scope.rootAgentId &&
    envelope.rootConversationId === scope.rootConversationId &&
    envelope.conversationId === scope.conversationId &&
    conversation.id === scope.conversationId &&
    envelope.event.runId === envelope.runId &&
    conversation.messages.some(
      (message) =>
        message.id === envelope.assistantMessageId && message.agentRun?.runId === envelope.runId
    )
  )
}

export function applyObserverLiveEnvelope(
  conversation: ChatConversation,
  scope: ObserverScope,
  envelope: AgentObserverEventEnvelope
): ChatConversation {
  if (!matchesObserverEnvelope(conversation, scope, envelope)) return conversation
  const message = conversation.messages.find(
    (candidate) =>
      candidate.id === envelope.assistantMessageId && candidate.agentRun?.runId === envelope.runId
  )
  if (!message) return conversation
  const next = applyAgentEventToChatMessage(message, envelope.event)
  if (next === message) return conversation
  return {
    ...conversation,
    messages: conversation.messages.map((candidate) =>
      candidate.id === message.id ? next : candidate
    )
  }
}

/**
 * Command Sessions outlive their parent Run and already carry exact durable owner identities.
 * Keep consuming that legacy stream as a narrow compatibility overlay until the process exits;
 * every other child event must use the Host-authenticated observer envelope.
 */
export function applyObserverCommandEvent(
  conversation: ChatConversation,
  scope: ObserverScope,
  event: AgentEvent
): ChatConversation {
  if (
    event.type !== 'command_started' &&
    event.type !== 'command_output' &&
    event.type !== 'command_exited' &&
    event.type !== 'command_interrupted'
  ) {
    return conversation
  }
  if (event.conversationId !== scope.conversationId || conversation.id !== scope.conversationId) {
    return conversation
  }
  const message = conversation.messages.find(
    (candidate) =>
      candidate.id === event.assistantMessageId && candidate.agentRun?.runId === event.runId
  )
  if (!message) return conversation
  const next = applyAgentEventToChatMessage(message, event)
  if (next === message) return conversation
  return {
    ...conversation,
    messages: conversation.messages.map((candidate) =>
      candidate.id === message.id ? next : candidate
    )
  }
}
