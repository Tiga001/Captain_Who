import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
  type SetStateAction
} from 'react'
import type { WorkflowInstance, WorkflowResponse } from '@mycopilot/protocol'
import {
  workflowNeighborNodes,
  type WorkflowNeighborNode
} from '../features/workflows/workflowNeighborNodes'
import type { ChatComposerDraft, ChatConversation } from '../features/chat/chatTypes'
import { loadComposerDrafts, loadConversationMetas } from '../features/storage/storageClient'
import { requestWorkflows } from '../features/workflows/workflowClient'
import { hostClient } from '../host/hostClient'

import type { WorkflowDraftPreferences } from './workflowDraftPreferenceSync'
export type { WorkflowDraftPreferences } from './workflowDraftPreferenceSync'

interface WorkflowWorkspaceOptions {
  conversationsRef: MutableRefObject<ChatConversation[]>
  hydrateDraft: (id: string, draft: ChatComposerDraft) => void
  readDraftPreferences: () => Readonly<Record<string, WorkflowDraftPreferences>>
  applyStoredDraftPreferences: (
    id: string,
    stored: Partial<WorkflowDraftPreferences>,
    expected: Partial<WorkflowDraftPreferences>,
    forcePersist?: boolean
  ) => Promise<void>
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
  conversationsRef,
  hydrateDraft,
  readDraftPreferences,
  applyStoredDraftPreferences,
  flushDraft,
  waitForConversationSaves,
  setConversations,
  applyDraftPreferences
}: WorkflowWorkspaceOptions) {
  const [instances, setInstances] = useState<WorkflowInstance[]>([])
  const refreshEpoch = useRef(0)
  const pendingSynchronization = useRef(new Set<string>())
  const pendingPreferences = useRef(
    new Map<string, { receipt: string; preferences: WorkflowDraftPreferences }>()
  )
  const appliedPreferences = useRef(new Map<string, string>())
  const synchronizationRequest = useRef<Promise<void> | null>(null)

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
      for (const id of response.affectedConversationIds ?? []) {
        const instance = response.instances?.find((item) =>
          item.bindings.some((binding) => binding.conversationId === id)
        )
        const binding = instance?.bindings.find((item) => item.conversationId === id)
        const node = instance?.definition.nodes.find((item) => item.id === binding?.nodeId)
        // Removed members keep their conversations and need no initialization.
        if (!instance || !binding || node?.kind !== 'agent') continue
        if (!node.modelConfigId) throw new Error('Organization binding initialization is missing')
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
  useEffect(() => {
    let disposed = false
    const seenManagementEvents = new Map<string, number>()
    const seenPreferenceEvents = new Map<string, number>()
    const seenPreferenceRevisions = new Map<string, number>()
    let freshPreferencePersistence: Promise<void> | null = null
    const pendingPreferenceChanges = new Map<
      string,
      {
        instanceId: string
        nodeId: string
        field: keyof WorkflowDraftPreferences
        expected: Readonly<Record<string, WorkflowDraftPreferences>>
      }
    >()
    const refresh = async () => {
      const epoch = ++refreshEpoch.current
      try {
        if (freshPreferencePersistence) await freshPreferencePersistence
        if (disposed || epoch !== refreshEpoch.current) return
        const response = await requestWorkflows({ operation: 'listInstances' })
        if (disposed || epoch !== refreshEpoch.current) return
        setInstances(response.instances ?? [])
        const currentIds = new Set(conversationsRef.current.map((conversation) => conversation.id))
        const missingIds = new Set(
          (response.instances ?? [])
            .flatMap((instance) => instance.bindings.map((binding) => binding.conversationId))
            .filter((id) => !currentIds.has(id))
        )
        const preferenceChanges = [...pendingPreferenceChanges.entries()]
        if (!missingIds.size && !preferenceChanges.length) return
        const [storedConversations, storedDrafts] = await Promise.all([
          missingIds.size ? loadConversationMetas() : Promise.resolve([]),
          loadComposerDrafts()
        ])
        if (disposed || epoch !== refreshEpoch.current) return
        const missingConversations = storedConversations.filter((conversation) =>
          missingIds.has(conversation.id)
        )
        // Host-created members already have durable drafts. Hydrate only missing drafts;
        // a catalog refresh must never overwrite preferences changed in a live composer.
        for (const conversation of missingConversations) {
          if (storedDrafts[conversation.id])
            hydrateDraft(conversation.id, storedDrafts[conversation.id])
        }
        const preferenceUpdates = new Map<string, Partial<WorkflowDraftPreferences>>()
        const consumedChanges: typeof preferenceChanges = []
        for (const [key, change] of preferenceChanges) {
          if (pendingPreferenceChanges.get(key) !== change) continue
          const instance = response.instances?.find((item) => item.id === change.instanceId)
          const id = instance?.bindings.find(
            (binding) => binding.nodeId === change.nodeId
          )?.conversationId
          if (id && !storedDrafts[id]) continue
          const expected = id ? change.expected[id] : undefined
          if (id && expected) {
            const update = preferenceUpdates.get(id) ?? {}
            if (change.field === 'modelId') update.modelId = expected.modelId
            else update.permissionMode = expected.permissionMode
            preferenceUpdates.set(id, update)
          }
          consumedChanges.push([key, change])
        }
        await Promise.all(
          [...preferenceUpdates].map(([id, expected]) =>
            applyStoredDraftPreferences(id, storedDrafts[id], expected)
          )
        )
        for (const [key, change] of consumedChanges) {
          if (pendingPreferenceChanges.get(key) === change) pendingPreferenceChanges.delete(key)
        }
        if (disposed || epoch !== refreshEpoch.current) return
        setConversations((current) => {
          const known = new Set(current.map((conversation) => conversation.id))
          return [
            ...current,
            ...missingConversations.filter((conversation) => !known.has(conversation.id))
          ]
        })
      } catch (error) {
        console.error('Failed to refresh organization membership', error)
      }
    }
    const changed = () => {
      void refresh()
    }
    void refresh()
    window.addEventListener('captain:workflows-changed', changed)
    window.addEventListener('focus', changed)
    const unsubscribe = hostClient.agent.onWorkflowRuntimeChanged?.((snapshot) => {
      let changed = false
      const expected = readDraftPreferences()
      const freshFields = new Set<string>()
      const freshSynchronizations: Promise<void>[] = []
      for (const update of snapshot.preferenceUpdates ?? []) {
        const desired: Partial<WorkflowDraftPreferences> = {}
        const currentExpected: Partial<WorkflowDraftPreferences> = {}
        for (const field of ['modelId', 'permissionMode'] as const) {
          if (!Object.hasOwn(update, field)) continue
          const key = `${snapshot.instanceId}:${update.nodeId}:${field}`
          freshFields.add(key)
          if (update.organizationRevision <= (seenPreferenceRevisions.get(key) ?? 0)) continue
          seenPreferenceRevisions.set(key, update.organizationRevision)
          pendingPreferenceChanges.delete(key)
          const current = expected[update.conversationId]
          if (!current) continue
          if (field === 'modelId') {
            desired.modelId = update.modelId
            currentExpected.modelId = current.modelId
          } else {
            desired.permissionMode = update.permissionMode
            currentExpected.permissionMode = current.permissionMode
          }
        }
        if (!Object.keys(desired).length) continue
        // Apply before any asynchronous read: subsequent queue saves must carry the new
        // preferences, and this full save follows any already pending older draft saves.
        freshSynchronizations.push(
          applyStoredDraftPreferences(update.conversationId, desired, currentExpected, true)
        )
        changed = true
      }
      for (const event of snapshot.events) {
        const field =
          event.kind === 'member_model_changed'
            ? 'modelId'
            : event.kind === 'member_permissions_changed'
              ? 'permissionMode'
              : null
        if (!field || !event.targetNodeId) continue
        const key = `${snapshot.instanceId}:${event.targetNodeId}:${field}`
        if (event.sequence <= (seenPreferenceEvents.get(key) ?? 0)) continue
        seenPreferenceEvents.set(key, event.sequence)
        if (freshFields.has(key)) continue
        pendingPreferenceChanges.set(key, {
          instanceId: snapshot.instanceId,
          nodeId: event.targetNodeId,
          field,
          expected
        })
        changed = true
      }
      const sequence = Math.max(
        snapshot.summary?.structureRevision ?? 0,
        ...snapshot.events
          .filter((event) => event.kind === 'members_changed')
          .map((event) => event.sequence)
      )
      if (sequence > (seenManagementEvents.get(snapshot.instanceId) ?? 0)) {
        seenManagementEvents.set(snapshot.instanceId, sequence)
        changed = true
      }
      if (!changed) return
      if (freshSynchronizations.length) {
        ++refreshEpoch.current
        const persistence = Promise.all([
          ...(freshPreferencePersistence ? [freshPreferencePersistence] : []),
          ...freshSynchronizations
        ])
          .then(() => undefined)
          .catch((error) => console.error('Failed to synchronize organization preferences', error))
        freshPreferencePersistence = persistence
        void persistence.then(() => {
          if (freshPreferencePersistence === persistence) freshPreferencePersistence = null
          if (!disposed) void refresh()
        })
      } else void refresh()
    })
    return () => {
      disposed = true
      window.removeEventListener('captain:workflows-changed', changed)
      window.removeEventListener('focus', changed)
      unsubscribe?.()
    }
  }, [
    conversationsRef,
    hydrateDraft,
    readDraftPreferences,
    applyStoredDraftPreferences,
    setConversations
  ])
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
  /** Other bound members, keyed by the active conversation. */
  const neighborNodes = useMemo(() => {
    const neighbors: Record<string, WorkflowNeighborNode[]> = {}
    for (const instance of instances) {
      const definition = instance.definition
      for (const binding of instance.bindings) {
        neighbors[binding.conversationId] = definition
          ? workflowNeighborNodes(definition, instance.bindings, binding.nodeId)
          : []
      }
    }
    return neighbors
  }, [instances])
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
