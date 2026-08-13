import { useEffect, useMemo, useRef, useState } from 'react'
import type { GitTurnDiffSummary } from '@mycopilot/protocol'
import { getGitTurnDiffSummaries } from '../gitReview/gitReviewClient'
import type { ChatConversation, ChatMessage } from './chatTypes'

const EMPTY_SUMMARIES = new Map<string, GitTurnDiffSummary>()

function isSettledAssistantMessage(message: ChatMessage): boolean {
  if (message.role !== 'assistant' || !message.agentRun || message.status === 'pending')
    return false
  const status = message.agentRun?.status
  return (
    !status ||
    status === 'completed' ||
    status === 'failed' ||
    status === 'cancelled' ||
    status === 'idle'
  )
}

interface TurnDiffSummaryState {
  conversationId: string | null
  summaries: Map<string, GitTurnDiffSummary>
}

/**
 * Loads the backend-authoritative net diff for all settled assistant turns in one
 * request. The map is derived from agent_turn_diffs and deliberately has no
 * tool-patch fallback, so overlapping edits cannot be counted more than once.
 */
export function useTurnDiffSummaries(
  conversation: ChatConversation,
  options: { enabled?: boolean } = {}
): ReadonlyMap<string, GitTurnDiffSummary> {
  const enabled = options.enabled ?? true
  const requestSequenceRef = useRef(0)
  const [state, setState] = useState<TurnDiffSummaryState>({
    conversationId: null,
    summaries: EMPTY_SUMMARIES
  })
  const requestKey = useMemo(
    () =>
      JSON.stringify(
        conversation.messages.filter(isSettledAssistantMessage).map((message) => message.id)
      ),
    [conversation.messages]
  )

  useEffect(() => {
    const requestSequence = requestSequenceRef.current + 1
    requestSequenceRef.current = requestSequence
    let cancelled = false
    const assistantMessageIds = JSON.parse(requestKey) as string[]

    if (!enabled || !conversation.projectId || assistantMessageIds.length === 0) {
      setState({
        conversationId: conversation.id,
        summaries: EMPTY_SUMMARIES
      })
      return
    }

    void getGitTurnDiffSummaries({
      assistantMessageIds,
      conversationId: conversation.id,
      projectId: conversation.projectId
    })
      .then((output) => {
        if (cancelled || requestSequenceRef.current !== requestSequence) return
        setState({
          conversationId: conversation.id,
          summaries: new Map(
            output.summaries.map((summary) => [summary.assistantMessageId, summary])
          )
        })
      })
      .catch(() => {
        if (cancelled || requestSequenceRef.current !== requestSequence) return
        setState((current) =>
          current.conversationId === conversation.id
            ? current
            : {
                conversationId: conversation.id,
                summaries: EMPTY_SUMMARIES
              }
        )
      })

    return () => {
      cancelled = true
    }
  }, [conversation.id, conversation.projectId, enabled, requestKey])

  return enabled && state.conversationId === conversation.id ? state.summaries : EMPTY_SUMMARIES
}
