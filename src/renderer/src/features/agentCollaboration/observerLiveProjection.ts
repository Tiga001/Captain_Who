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

export function isObserverTextEvent(event: AgentEvent): boolean {
  return (
    event.type === 'message_delta' ||
    event.type === 'message' ||
    event.type === 'message_stream_started' ||
    event.type === 'message_stream_reset' ||
    event.type === 'message_stream_committed'
  )
}

/** A snapshot's complete prefix replaces provisional text, never durable history before it. */
export function applyObserverStreamSnapshot(
  conversation: ChatConversation,
  snapshot: AgentObserverLiveStreamSnapshot | undefined
): ChatConversation {
  if (!snapshot?.stream) return conversation
  const { stream, runId, assistantMessageId } = snapshot
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
      // A narration may already have committed in SQLite just before its stream-commit event.
      // The start boundary identifies that same narration without guessing from matching text.
      const isCurrentStream = (item: (typeof run.timeline)[number]) =>
        item.type === 'message' &&
        (item.streamId === stream.streamId ||
          (item.traceSequence !== undefined && item.traceSequence >= stream.traceBoundarySequence))
      const insertionIndex = run.timeline.findIndex(isCurrentStream)
      const checkpoints = { ...run.messageStreamCheckpoints }
      delete checkpoints[stream.streamId]
      let next: ChatMessage = {
        ...message,
        agentRun: {
          ...run,
          timeline: run.timeline.filter((item) => !isCurrentStream(item)),
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
      return next
    })
  }
}

/** Only text uses this cursor: other event families keep their own durable identities. */
export function applyObserverEnvelopeWithCursor(
  conversation: ChatConversation,
  scope: ObserverScope,
  envelope: AgentObserverEventEnvelope,
  position: ObserverStreamPosition | null
): { conversation: ChatConversation; position: ObserverStreamPosition | null } {
  const cursor = envelope.streamCursor
  const textEvent = isObserverTextEvent(envelope.event)
  if (
    textEvent &&
    cursor &&
    position?.runId === envelope.runId &&
    position.assistantMessageId === envelope.assistantMessageId &&
    position.cursor.generation === cursor.generation &&
    position.cursor.sequence >= cursor.sequence
  ) {
    return { conversation, position }
  }
  const next = applyObserverLiveEnvelope(conversation, scope, envelope)
  return {
    conversation: next,
    position:
      textEvent && cursor && next !== conversation
        ? { runId: envelope.runId, assistantMessageId: envelope.assistantMessageId, cursor }
        : position
  }
}

export function applyObserverLiveEnvelope(
  conversation: ChatConversation,
  scope: ObserverScope,
  envelope: AgentObserverEventEnvelope
): ChatConversation {
  if (
    envelope.agentId !== scope.agentId ||
    envelope.rootAgentId !== scope.rootAgentId ||
    envelope.rootConversationId !== scope.rootConversationId ||
    envelope.conversationId !== scope.conversationId ||
    conversation.id !== scope.conversationId ||
    envelope.event.runId !== envelope.runId
  ) {
    return conversation
  }
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
