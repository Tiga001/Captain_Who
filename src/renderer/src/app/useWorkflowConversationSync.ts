import type { WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import { useEffect, type Dispatch, type MutableRefObject, type SetStateAction } from 'react'
import type { ChatConversation } from '../features/chat/chatTypes'
import { isAssistantMessageGenerating } from '../features/chat/assistantGeneration'
import { loadConversation } from '../features/storage/storageClient'
import { requestWorkflows } from '../features/workflows/workflowClient'
import { hostClient } from '../host/hostClient'
import { mergeHumanInteractionConversation } from './useHumanInteractionConversationSync'
import { useAccountAuth } from '../features/auth/AccountAuthContext'

const MAX_RECOVERY_READS = 4

/** Attach Host-created turns to the UI; preserve in-flight deltas and never start a turn here. */
export function useWorkflowConversationSync({
  conversationsRef,
  setConversations,
  pendingActionsHydratedRef,
  visibleConversationIdRef,
  enqueueConversationMetaSave
}: {
  conversationsRef: MutableRefObject<ChatConversation[]>
  setConversations: Dispatch<SetStateAction<ChatConversation[]>>
  pendingActionsHydratedRef: MutableRefObject<Set<string>>
  visibleConversationIdRef: MutableRefObject<string | null>
  enqueueConversationMetaSave: (conversation: ChatConversation) => void
}) {
  const auth = useAccountAuth()
  // The workspace stays mounted under the login overlay; recovery state must not cross accounts.
  const authScope = auth ? `${auth.state.status}:${auth.state.profile?.userId ?? ''}` : null
  useEffect(() => {
    if (!hostClient.agent.onWorkflowRuntimeChanged) return
    let disposed = false
    let recovering = false
    const pending = new Set<string>()
    const queued = new Set<string>()
    const dirty = new Set<string>()
    const failed = new Set<string>()
    const versions = new Map<string, string>()
    const terminalSequences = new Map<string, number>()
    const conversationSequences = new Map<string, number>()
    let indexedConversations: ChatConversation[] | null = null
    let conversationsById = new Map<string, ChatConversation>()
    const currentConversations = () => {
      if (indexedConversations !== conversationsRef.current) {
        indexedConversations = conversationsRef.current
        conversationsById = new Map(indexedConversations.map((item) => [item.id, item]))
        for (const id of conversationSequences.keys()) {
          if (!conversationsById.has(id)) conversationSequences.delete(id)
        }
      }
      return conversationsById
    }
    const refresh = async (id: string): Promise<void> => {
      if (disposed) return
      if (pending.has(id)) {
        dirty.add(id)
        return
      }
      const baseline = currentConversations().get(id)
      if (!baseline) return
      pending.add(id)
      try {
        const stored = await loadConversation(id)
        if (!disposed && stored) {
          failed.delete(id)
          pendingActionsHydratedRef.current.delete(id)
          let markedRead: ChatConversation | undefined
          setConversations((current) =>
            current.map((conversation) => {
              if (conversation.id !== id) return conversation
              const merged = mergeHumanInteractionConversation(conversation, stored, baseline)
              if (
                visibleConversationIdRef.current === id &&
                document.visibilityState !== 'hidden'
              ) {
                merged.unreadAt = null
                if (stored.unreadAt) markedRead = merged
              } else if (conversation.unreadAt === baseline.unreadAt) {
                merged.unreadAt = Math.max(conversation.unreadAt ?? 0, stored.unreadAt ?? 0) || null
              }
              return merged
            })
          )
          if (markedRead) enqueueConversationMetaSave(markedRead)
        }
      } catch {
        failed.add(id)
        /* Focus/poll recovery retries reads without duplicating sends. */
      } finally {
        pending.delete(id)
        if (dirty.delete(id) && !disposed) queued.add(id)
        drain()
      }
    }
    const drain = () => {
      if (disposed) return
      while (queued.size && pending.size < MAX_RECOVERY_READS) {
        const id = queued.values().next().value!
        queued.delete(id)
        void refresh(id)
      }
    }
    const schedule = (id: string) => {
      if (pending.has(id)) dirty.add(id)
      else queued.add(id)
      drain()
    }
    const accept = (snapshot: WorkflowRuntimeSnapshot) => {
      const affected = new Set<string>()
      const conversations = currentConversations()
      if (snapshot.summary) {
        for (const change of snapshot.summary.conversationChanges) {
          if (
            !conversations.has(change.conversationId) ||
            change.sequence <= (conversationSequences.get(change.conversationId) ?? 0)
          )
            continue
          // Mail event sequences are globally monotonic, including moves between organizations.
          // Unknown/not-yet-hydrated conversations are never acknowledged; recovery sees them again.
          conversationSequences.set(change.conversationId, change.sequence)
          affected.add(change.conversationId)
        }
        for (const id of affected) schedule(id)
        return
      }
      const inputsById = new Map(snapshot.inputs.map((input) => [input.id, input]))
      for (const input of snapshot.inputs) {
        if (!input.conversationId || !input.deliveryId) continue
        // Metadata hydration can lag the first Host notification. Do not acknowledge a receipt
        // until there is a local conversation to attach it to; recovery will observe it again.
        if (!conversations.has(input.conversationId)) continue
        const version = JSON.stringify([input.runId, input.deliveryId, input.status])
        if (versions.get(input.id) === version) continue
        versions.set(input.id, version)
        if (versions.size > 4096) versions.delete(versions.keys().next().value!)
        affected.add(input.conversationId)
      }
      // A completed Run leaves its applied receipt unchanged. Its durable terminal event still
      // requires a final reload, including unread reconciliation for a currently visible chat.
      for (const event of snapshot.events) {
        if (
          event.kind !== 'run_completed' ||
          !event.inputId ||
          event.sequence <= (terminalSequences.get(event.inputId) ?? 0)
        )
          continue
        const input = inputsById.get(event.inputId)
        if (!input?.conversationId || !conversations.has(input.conversationId)) continue
        terminalSequences.set(event.inputId, event.sequence)
        if (terminalSequences.size > 4096)
          terminalSequences.delete(terminalSequences.keys().next().value!)
        affected.add(input.conversationId)
      }
      for (const id of affected) schedule(id)
    }
    const recover = async () => {
      if (disposed || recovering || document.visibilityState === 'hidden') return
      recovering = true
      try {
        const response = await requestWorkflows({ operation: 'listInstances' })
        if (disposed) return
        for (const instance of response.instances ?? []) {
          if (disposed) break
          const runtime = await requestWorkflows({
            operation: 'runtimeSnapshot',
            instanceId: instance.id,
            summaryOnly: true
          })
          if (!disposed && runtime.runtime) {
            accept(runtime.runtime)
            // A terminal Run may not change the delivery receipt; refresh its current projection.
            const known = currentConversations()
            const ids = new Set<string>()
            if (runtime.runtime.summary) {
              for (const change of runtime.runtime.summary.conversationChanges) {
                if (known.has(change.conversationId)) ids.add(change.conversationId)
              }
            } else {
              for (const input of runtime.runtime.inputs) {
                if (input.deliveryId && input.conversationId && known.has(input.conversationId))
                  ids.add(input.conversationId)
              }
            }
            for (const id of ids) {
              const current = known.get(id)
              if (
                id &&
                !pending.has(id) &&
                (failed.has(id) || current?.messages.some(isAssistantMessageGenerating))
              )
                schedule(id)
            }
          }
        }
      } catch {
        /* Host recovery can temporarily interrupt either read. */
      } finally {
        recovering = false
      }
    }
    const unsubscribe = hostClient.agent.onWorkflowRuntimeChanged(accept)
    const onRecover = () => {
      void recover()
    }
    window.addEventListener('focus', onRecover)
    document.addEventListener('visibilitychange', onRecover)
    const interval = setInterval(onRecover, 15_000)
    onRecover()
    return () => {
      disposed = true
      queued.clear()
      unsubscribe()
      clearInterval(interval)
      window.removeEventListener('focus', onRecover)
      document.removeEventListener('visibilitychange', onRecover)
    }
  }, [
    authScope,
    conversationsRef,
    pendingActionsHydratedRef,
    setConversations,
    visibleConversationIdRef,
    enqueueConversationMetaSave
  ])
}
