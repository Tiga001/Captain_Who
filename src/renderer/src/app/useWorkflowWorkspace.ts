import { useCallback, useEffect, useMemo, useRef, useState, type SetStateAction } from 'react'
import type { WorkflowInstance, WorkflowRecord, WorkflowResponse } from '@mycopilot/protocol'
import {
  workflowNeighborNodes,
  type WorkflowNeighborNode
} from '../features/workflows/workflowNeighborNodes'
import type { ChatComposerDraft, ChatConversation } from '../features/chat/chatTypes'
import { loadComposerDrafts, loadConversationMetas } from '../features/storage/storageClient'
import { requestWorkflows } from '../features/workflows/workflowClient'

export type WorkflowDraftPreferences = Pick<ChatComposerDraft, 'modelId' | 'permissionMode'>

interface WorkflowWorkspaceOptions {
  flushDraft: (id: string) => Promise<void>
  waitForConversationSaves: (id: string) => Promise<void>
  setConversations: (value: SetStateAction<ChatConversation[]>) => void
  applyDraftPreferences: (
    id: string,
    preferences: WorkflowDraftPreferences,
    fallback?: ChatComposerDraft
  ) => Promise<boolean>
}

/** Keeps workflow membership and one-time next-turn binding preferences in sync with durable storage. */
export function useWorkflowWorkspace({
  flushDraft,
  waitForConversationSaves,
  setConversations,
  applyDraftPreferences
}: WorkflowWorkspaceOptions) {
  const [instances, setInstances] = useState<WorkflowInstance[]>([])
  const [records, setRecords] = useState<WorkflowRecord[]>([])
  const refreshEpoch = useRef(0)
  const pendingSynchronization = useRef(new Set<string>())
  const pendingPreferences = useRef(
    new Map<string, { receipt: string; preferences: WorkflowDraftPreferences }>()
  )
  const appliedPreferences = useRef(new Map<string, string>())
  const synchronizationRequest = useRef<Promise<void> | null>(null)
  useEffect(() => {
    let disposed = false
    const refresh = async () => {
      const epoch = ++refreshEpoch.current
      try {
        const response = await requestWorkflows({ operation: 'listInstances' })
        if (!disposed && epoch === refreshEpoch.current) {
          setInstances(response.instances ?? [])
          setRecords(response.records ?? [])
        }
      } catch (error) {
        console.error('Failed to refresh workflow membership', error)
      }
    }
    void refresh()
    window.addEventListener('captain:workflows-changed', refresh)
    return () => {
      disposed = true
      window.removeEventListener('captain:workflows-changed', refresh)
    }
  }, [])

  const beforeCommit = useCallback(
    async (ids: readonly string[]) => {
      await Promise.all(
        [...new Set(ids)].map(async (id) => {
          await flushDraft(id)
          await waitForConversationSaves(id)
        })
      )
    },
    [flushDraft, waitForConversationSaves]
  )

  const retrySynchronization = useCallback(async () => {
    if (synchronizationRequest.current) return synchronizationRequest.current
    const affected = new Set(pendingSynchronization.current)
    if (!affected.size) return
    const request = (async () => {
      const applyPreferences = async (id: string, fallback?: ChatComposerDraft) => {
        const pending = pendingPreferences.current.get(id)
        if (!pending || appliedPreferences.current.get(id) === pending.receipt) return
        if (await applyDraftPreferences(id, pending.preferences, fallback)) {
          appliedPreferences.current.set(id, pending.receipt)
        }
      }
      // Apply through the live composer path before awaiting reads. Run callbacks can continue
      // updating queues while binding; those writes must already carry the new preferences.
      await Promise.all([...affected].map((id) => applyPreferences(id)))
      const [storedConversations, storedDrafts] = await Promise.all([
        loadConversationMetas(),
        loadComposerDrafts()
      ])
      setConversations((current) => {
        const currentById = new Map(current.map((conversation) => [conversation.id, conversation]))
        for (const stored of storedConversations) {
          if (!affected.has(stored.id)) continue
          // Binding never changes existing metadata or the active Run's selected model.
          if (!currentById.has(stored.id)) currentById.set(stored.id, stored)
        }
        return [...currentById.values()]
      })
      await Promise.all([...affected].map((id) => applyPreferences(id, storedDrafts[id])))
      for (const id of affected) {
        const pending = pendingPreferences.current.get(id)
        if (pending && appliedPreferences.current.get(id) !== pending.receipt) {
          throw new Error('Initialized conversation draft is missing')
        }
        pendingSynchronization.current.delete(id)
        pendingPreferences.current.delete(id)
      }
    })()
    synchronizationRequest.current = request
    try {
      await request
    } finally {
      synchronizationRequest.current = null
    }
  }, [setConversations, applyDraftPreferences])

  const hasPendingSynchronization = useCallback(() => pendingSynchronization.current.size > 0, [])
  const committed = useCallback(
    async (response: WorkflowResponse) => {
      ++refreshEpoch.current
      if (response.instances) setInstances(response.instances)
      if (response.records) setRecords(response.records)
      for (const id of response.affectedConversationIds ?? []) {
        const instance = response.instances?.find((item) =>
          item.bindings.some((binding) => binding.conversationId === id)
        )
        const binding = instance?.bindings.find((item) => item.conversationId === id)
        const node = response.records
          .find((item) => item.definition.id === instance?.templateId)
          ?.definition.nodes.find((item) => item.id === binding?.nodeId)
        if (!instance || !binding || node?.kind !== 'agent' || !node.modelConfigId) {
          throw new Error('Workflow binding initialization is missing')
        }
        pendingPreferences.current.set(id, {
          receipt: `${instance.id}:${instance.revision}:${binding.nodeId}`,
          preferences: { modelId: node.modelConfigId, permissionMode: node.permissionMode }
        })
        pendingSynchronization.current.add(id)
      }
      await retrySynchronization()
    },
    [retrySynchronization]
  )
  const { memberships, allMemberships } = useMemo(() => {
    type Membership = { id: string; name: string; color: string }
    const memberships: Record<string, Membership> = {}
    const allMemberships: Record<string, Membership> = {}
    for (const instance of instances) {
      for (const binding of instance.bindings) {
        const membership = {
          id: instance.id,
          name: instance.name,
          color: instance.color
        }
        allMemberships[binding.conversationId] = membership
        if (instance.enabled) memberships[binding.conversationId] = membership
      }
    }
    return { memberships, allMemberships }
  }, [instances])
  /** Upstream and downstream bound agents, keyed by the conversation bound to the node. */
  const neighborNodes = useMemo(() => {
    const neighbors: Record<
      string,
      { upstream: WorkflowNeighborNode[]; downstream: WorkflowNeighborNode[] }
    > = {}
    for (const instance of instances) {
      const definition = records.find(
        (record) => record.definition.id === instance.templateId
      )?.definition
      for (const binding of instance.bindings) {
        neighbors[binding.conversationId] = definition
          ? workflowNeighborNodes(definition, instance.bindings, binding.nodeId)
          : { upstream: [], downstream: [] }
      }
    }
    return neighbors
  }, [instances, records])
  return {
    beforeCommit,
    committed,
    memberships,
    allMemberships,
    neighborNodes,
    hasPendingSynchronization,
    retrySynchronization
  }
}
