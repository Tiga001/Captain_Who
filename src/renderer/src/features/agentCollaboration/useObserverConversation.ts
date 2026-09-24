import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { AgentEvent, AgentObserverEventEnvelope } from '@mycopilot/protocol'
import { onAgentEvent } from '../agent/agentClient'
import type { ChatConversation } from '../chat/chatTypes'
import {
  loadCollaborationObserverConversation,
  onCollaborationObserverEvent
} from './collaborationClient'
import { mapObserverConversationToChat } from './observerConversationAdapter'
import {
  applyObserverCommandEvent,
  applyObserverEnvelopeWithCursor,
  type ObserverScope,
  type ObserverStreamPosition
} from './observerLiveProjection'

export interface UseObserverConversationInput {
  agentId: string
  conversationId: string
  /** Change this root-scoped durable sequence to refresh an active child safely. */
  invalidationVersion?: number | string
  rootAgentId: string
  rootConversationId: string
}

export interface ObserverConversationState {
  conversation: ChatConversation | null
  error: string | null
  loading: boolean
  reload: () => void
}

interface ScopedObserverConversationState extends Omit<ObserverConversationState, 'reload'> {
  conversationId: string
  rootConversationId: string
  scopeKey: string
  streamPosition: ObserverStreamPosition | null
}

type ObserverLoadWindow = {
  generation: number
  items: ObserverJournalItem[]
  overflowed: boolean
  scopeKey: string
}

type ObserverJournalItem =
  | {
      sequence: number
      type: 'observer'
      envelope: AgentObserverEventEnvelope
      textDeltas?: Array<{ sequence: number; delta: string }>
    }
  | { sequence: number; type: 'command'; event: AgentEvent }

const MAX_OBSERVER_LIVE_JOURNAL = 512

export function useObserverConversation({
  agentId,
  conversationId,
  invalidationVersion = 0,
  rootAgentId,
  rootConversationId
}: UseObserverConversationInput): ObserverConversationState {
  const scopeKey = `${rootAgentId}\u0000${rootConversationId}\u0000${agentId}\u0000${conversationId}`
  const requestGenerationRef = useRef(0)
  const arrivalSequenceRef = useRef(0)
  const activeLoadWindowRef = useRef<ObserverLoadWindow | null>(null)
  const overflowInvalidationRef = useRef<number | string | null>(null)
  const [reloadVersion, setReloadVersion] = useState(0)
  const [state, setState] = useState<ScopedObserverConversationState>({
    conversation: null,
    conversationId,
    error: null,
    loading: true,
    rootConversationId,
    scopeKey,
    streamPosition: null
  })
  const reload = useCallback(() => {
    overflowInvalidationRef.current = null
    setReloadVersion((version) => version + 1)
  }, [])
  const scope: ObserverScope = useMemo(
    () => ({ agentId, rootAgentId, rootConversationId, conversationId }),
    [agentId, conversationId, rootAgentId, rootConversationId]
  )
  useEffect(() => {
    activeLoadWindowRef.current = null
    overflowInvalidationRef.current = null
    arrivalSequenceRef.current = 0
    const apply = (
      updater: (conversation: ChatConversation, scope: ObserverScope) => ChatConversation
    ) => {
      setState((current) => {
        if (
          !current.conversation ||
          current.rootConversationId !== scope.rootConversationId ||
          current.conversationId !== scope.conversationId
        ) {
          return current
        }
        const conversation = updater(current.conversation, scope)
        return conversation === current.conversation ? current : { ...current, conversation }
      })
    }
    const unsubscribeObserver = onCollaborationObserverEvent((envelope) => {
      if (
        envelope.rootAgentId !== scope.rootAgentId ||
        envelope.rootConversationId !== scope.rootConversationId ||
        envelope.agentId !== scope.agentId ||
        envelope.conversationId !== scope.conversationId
      ) {
        return
      }
      const sequence = ++arrivalSequenceRef.current
      const journalItem: ObserverJournalItem = { sequence, type: 'observer', envelope }
      appendToActiveLoadWindow(activeLoadWindowRef.current, scopeKey, journalItem)
      setState((current) => {
        if (!current.conversation || current.scopeKey !== scopeKey) return current
        const projected = applyObserverEnvelopeWithCursor(
          current.conversation,
          scope,
          envelope,
          current.streamPosition
        )
        return projected.conversation === current.conversation &&
          projected.position === current.streamPosition
          ? current
          : { ...current, conversation: projected.conversation, streamPosition: projected.position }
      })
    })
    const unsubscribeCommands = onAgentEvent((event) => {
      if (
        event.type !== 'command_started' &&
        event.type !== 'command_output' &&
        event.type !== 'command_exited' &&
        event.type !== 'command_interrupted'
      ) {
        return
      }
      if (event.conversationId !== scope.conversationId) return
      const sequence = ++arrivalSequenceRef.current
      const journalItem: ObserverJournalItem = { sequence, type: 'command', event }
      appendToActiveLoadWindow(activeLoadWindowRef.current, scopeKey, journalItem)
      apply((conversation, scope) => applyObserverCommandEvent(conversation, scope, event))
    })
    return () => {
      unsubscribeObserver()
      unsubscribeCommands()
    }
  }, [scope, scopeKey])

  useEffect(() => {
    const generation = requestGenerationRef.current + 1
    requestGenerationRef.current = generation
    let cancelled = false
    const blockedOnSameInvalidation = overflowInvalidationRef.current === invalidationVersion
    if (blockedOnSameInvalidation) return
    overflowInvalidationRef.current = null
    const loadWindow: ObserverLoadWindow = {
      generation,
      items: [],
      overflowed: false,
      scopeKey
    }
    activeLoadWindowRef.current = loadWindow
    setState((current) => {
      const sameScope = current.scopeKey === scopeKey
      return {
        conversation: sameScope ? current.conversation : null,
        conversationId,
        error: null,
        loading: true,
        rootConversationId,
        scopeKey,
        streamPosition: sameScope ? current.streamPosition : null
      }
    })

    void loadCollaborationObserverConversation({ rootConversationId, conversationId })
      .then((observer) => {
        if (cancelled || requestGenerationRef.current !== generation) return
        if (activeLoadWindowRef.current === loadWindow) activeLoadWindowRef.current = null
        if (!observer) throw new Error('Observer conversation is unavailable.')
        if (
          observer.rootConversationId !== rootConversationId ||
          observer.conversationId !== conversationId
        ) {
          throw new Error('Observer conversation identity mismatch.')
        }
        if (loadWindow.overflowed) {
          // A token stream can outrun the bounded request window before narration is durable.
          // Never replace a visible overlay (or expose an incomplete initial snapshot) with that
          // stale response. The next target-scoped durable invalidation/resync retries hydration.
          overflowInvalidationRef.current = invalidationVersion
          setState((current) =>
            current.conversation
              ? { ...current, error: null, loading: false }
              : {
                  ...current,
                  error: 'Observer hydration was overtaken by the live stream. Retry to recover.',
                  loading: false
                }
          )
          return
        }
        let conversation = mapObserverConversationToChat(observer)
        let streamPosition: ObserverStreamPosition | null = observer.liveStream ?? null
        for (const item of loadWindow.items) {
          if (item.type === 'observer') {
            const envelopes =
              item.textDeltas &&
              item.envelope.event.type === 'message_delta' &&
              item.envelope.streamCursor
                ? item.textDeltas.map((part) => ({
                    ...item.envelope,
                    streamCursor: { ...item.envelope.streamCursor!, sequence: part.sequence },
                    event: {
                      ...item.envelope.event,
                      type: 'message_delta' as const,
                      runId: item.envelope.runId,
                      delta: part.delta
                    }
                  }))
                : [item.envelope]
            for (const envelope of envelopes) {
              const projected = applyObserverEnvelopeWithCursor(
                conversation,
                scope,
                envelope,
                streamPosition
              )
              conversation = projected.conversation
              streamPosition = projected.position
            }
          } else {
            conversation = applyObserverCommandEvent(conversation, scope, item.event)
          }
        }
        setState({
          conversation,
          conversationId,
          error: null,
          loading: false,
          rootConversationId,
          scopeKey,
          streamPosition
        })
      })
      .catch((error: unknown) => {
        if (cancelled || requestGenerationRef.current !== generation) return
        if (activeLoadWindowRef.current === loadWindow) activeLoadWindowRef.current = null
        setState((current) => ({
          // A same-scope refresh is only an invalidation of freshness, not of the already
          // authorized snapshot. Keep it readable while exposing a retry. Initial loads and
          // exact identity changes still fail closed with no previous child content.
          conversation: current.scopeKey === scopeKey ? current.conversation : null,
          conversationId,
          error: error instanceof Error ? error.message : String(error),
          loading: false,
          rootConversationId,
          scopeKey,
          streamPosition: current.scopeKey === scopeKey ? current.streamPosition : null
        }))
      })

    return () => {
      cancelled = true
      if (activeLoadWindowRef.current === loadWindow) activeLoadWindowRef.current = null
    }
  }, [
    agentId,
    conversationId,
    invalidationVersion,
    reloadVersion,
    rootAgentId,
    rootConversationId,
    scope,
    scopeKey
  ])

  if (state.scopeKey !== scopeKey) {
    return { conversation: null, error: null, loading: true, reload }
  }

  return { conversation: state.conversation, error: state.error, loading: state.loading, reload }
}

function appendToActiveLoadWindow(
  window: ObserverLoadWindow | null,
  scopeKey: string,
  item: ObserverJournalItem
): void {
  if (!window || window.scopeKey !== scopeKey) return
  const previous = window.items.at(-1)
  if (
    item.type === 'observer' &&
    previous?.type === 'observer' &&
    item.envelope.event.type === 'message_delta' &&
    previous.envelope.event.type === 'message_delta' &&
    item.envelope.streamCursor &&
    previous.envelope.streamCursor &&
    item.envelope.runId === previous.envelope.runId &&
    item.envelope.assistantMessageId === previous.envelope.assistantMessageId &&
    item.envelope.streamCursor.generation === previous.envelope.streamCursor.generation &&
    item.envelope.event.streamId === previous.envelope.event.streamId
  ) {
    // Count a contiguous text stream as one journal item, retaining exact delta cursors so a
    // snapshot cut in the middle can replay only its unseen suffix. Long answers cannot exhaust
    // the event-count budget merely because the provider emits very small token fragments.
    previous.textDeltas ??= [
      { sequence: previous.envelope.streamCursor.sequence, delta: previous.envelope.event.delta }
    ]
    previous.textDeltas.push({
      sequence: item.envelope.streamCursor.sequence,
      delta: item.envelope.event.delta
    })
    return
  }
  if (window.items.length >= MAX_OBSERVER_LIVE_JOURNAL) {
    window.overflowed = true
    return
  }
  window.items.push(item)
}
