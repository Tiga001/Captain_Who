import { useEffect, useMemo, useState } from 'react'
import type { AgentPromptPredictionInput } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export function useNextUserMessagePrediction({
  conversationId,
  assistantMessageId,
  modelId,
  configurationRevision,
  accountId,
  eligible
}: {
  conversationId?: string
  assistantMessageId?: string
  modelId?: string
  configurationRevision: string | null
  accountId?: string
  eligible: boolean
}) {
  const view = useMemo(
    () => ({ conversationId, assistantMessageId }),
    [conversationId, assistantMessageId]
  )
  const [dismissedView, setDismissedView] = useState<typeof view | null>(null)
  const target = useMemo(() => {
    if (
      !eligible ||
      view === dismissedView ||
      !view.conversationId ||
      !view.assistantMessageId ||
      !modelId
    )
      return null
    return {
      request: {
        conversationId: view.conversationId,
        assistantMessageId: view.assistantMessageId,
        modelId
      },
      configurationRevision,
      accountId
    }
  }, [eligible, view, dismissedView, modelId, configurationRevision, accountId])
  const [result, setResult] = useState<{ target: typeof target; suggestion: string | null } | null>(
    null
  )

  useEffect(() => {
    if (!target) return
    let subscribed = true
    let retry: ReturnType<typeof setTimeout>
    async function request(params: AgentPromptPredictionInput, retryOnce: boolean) {
      try {
        const { suggestion } = await hostClient.agent.predictNextUserMessage(params)
        if (subscribed) setResult({ target, suggestion })
      } catch {
        // A renderer terminal event may precede its durable checkpoint. Keep ordinary input usable.
        if (subscribed && retryOnce) retry = setTimeout(() => void request(params, false), 1000)
      }
    }
    void request(target.request, true)
    return () => {
      // The Host shares this computation. Navigating away only detaches this view.
      subscribed = false
      clearTimeout(retry)
    }
  }, [target])

  return {
    suggestion: result && result.target === target ? result.suggestion : null,
    dismiss: () => {
      if (view.assistantMessageId) setDismissedView(view)
    }
  }
}
