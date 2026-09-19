import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  AgentContextWindowSnapshot,
  AgentPermissions,
  SkillSelection
} from '@mycopilot/protocol'
import { getContextWindowSnapshot } from '../agent/agentClient'
import { hostClient } from '../../host/hostClient'
import { resolveChatPermissions } from '../chat/chatPermissions'
import type { ChatPermissionMode } from '../chat/chatTypes'
import type { UiPreferencesSnapshot } from '../storage/storageClient'

function snapshotKey(scopeId: string, modelId: string): string {
  return `${scopeId}\u0000${modelId}`
}

interface UseContextWindowSnapshotsOptions {
  conversationId?: string
  customPermissions: UiPreferencesSnapshot['customPermissions']
  enabled: boolean
  isRunning?: boolean
  refreshKey?: string
  modelId: string | null
  permissionMode: ChatPermissionMode
  projectId: string | null
  scopeId: string
  skills: SkillSelection[]
}

interface ContextWindowSnapshotRequestDescriptor {
  conversationId: string | null
  modelId: string | null
  permissions: AgentPermissions
  projectId: string | null
  scopeId: string
  skills: SkillSelection[]
}

function requestDescriptorKey({
  conversationId,
  customPermissions,
  modelId,
  permissionMode,
  projectId,
  scopeId,
  skills
}: Omit<UseContextWindowSnapshotsOptions, 'enabled' | 'isRunning' | 'refreshKey'>): string {
  const permissions = resolveChatPermissions(permissionMode, customPermissions)
  const descriptor: ContextWindowSnapshotRequestDescriptor = {
    conversationId: conversationId ?? null,
    modelId,
    permissions: {
      read: permissions.read,
      write: permissions.write,
      command: permissions.command,
      commandSafety: permissions.commandSafety,
      patch: permissions.patch,
      builtinExecution: permissions.builtinExecution
    },
    projectId,
    scopeId,
    skills: skills.map(({ id, revision }) => ({ id, revision }))
  }
  return JSON.stringify(descriptor)
}

export function useContextWindowSnapshots({
  conversationId,
  customPermissions,
  enabled,
  isRunning = false,
  refreshKey,
  modelId,
  permissionMode,
  projectId,
  scopeId,
  skills
}: UseContextWindowSnapshotsOptions) {
  const [collaborationSettingsRevision, setCollaborationSettingsRevision] = useState(0)
  const [promptPreferencesRevision, setPromptPreferencesRevision] = useState(0)
  useEffect(
    () =>
      hostClient.agent.onPromptPreferencesChanged(() => {
        setPromptPreferencesRevision((revision) => revision + 1)
      }),
    []
  )
  useEffect(
    () =>
      hostClient.agent.onCollaborationSettingsChanged((settings) => {
        setCollaborationSettingsRevision((current) => Math.max(current, settings.revision))
      }),
    []
  )
  const requestSequenceRef = useRef(0)
  const eventSequenceRef = useRef<Map<string, number>>(new Map())
  const [snapshots, setSnapshots] = useState<Record<string, AgentContextWindowSnapshot>>({})
  const activeSnapshotKey = modelId ? snapshotKey(scopeId, modelId) : null
  const activeSnapshot = enabled && activeSnapshotKey ? snapshots[activeSnapshotKey] : undefined
  const requestKey = requestDescriptorKey({
    conversationId,
    customPermissions,
    modelId,
    permissionMode,
    projectId,
    scopeId,
    skills
  })

  useEffect(() => {
    // During a Run, only Host events describe the model's actual context and frozen permissions.
    // Composer settings are for a future Turn; estimating with them would replace the current
    // Run's usage with an unrelated preview. This also retires any pre-Run inspection in flight.
    if (!enabled || isRunning) return undefined

    const request = JSON.parse(requestKey) as ContextWindowSnapshotRequestDescriptor
    if (!request.modelId) return undefined

    const requestSequence = requestSequenceRef.current + 1
    requestSequenceRef.current = requestSequence
    const requestedSnapshotKey = snapshotKey(request.scopeId, request.modelId)
    const eventSequenceAtRequest = eventSequenceRef.current.get(requestedSnapshotKey) ?? 0
    let cancelled = false

    void getContextWindowSnapshot({
      conversationId: request.conversationId ?? undefined,
      projectId: request.projectId,
      modelId: request.modelId,
      skills: request.skills.length > 0 ? request.skills : undefined,
      permissions: request.permissions
    })
      .then(({ modelConfigId, snapshot }) => {
        if (cancelled || requestSequenceRef.current !== requestSequence) return
        if ((eventSequenceRef.current.get(requestedSnapshotKey) ?? 0) !== eventSequenceAtRequest) {
          return
        }
        setSnapshots((current) => {
          if (snapshot && modelConfigId === request.modelId) {
            return { ...current, [requestedSnapshotKey]: snapshot }
          }
          if (!(requestedSnapshotKey in current)) return current
          const next = { ...current }
          delete next[requestedSnapshotKey]
          return next
        })
      })
      .catch((error) => {
        if (!cancelled) console.error('Failed to inspect context window', error)
      })

    return () => {
      cancelled = true
    }
  }, [
    enabled,
    isRunning,
    refreshKey,
    requestKey,
    collaborationSettingsRevision,
    promptPreferencesRevision
  ])

  const recordSnapshot = useCallback(
    (eventScopeId: string, eventModelConfigId: string, snapshot: AgentContextWindowSnapshot) => {
      if (!enabled) return
      const eventSnapshotKey = snapshotKey(eventScopeId, eventModelConfigId)
      eventSequenceRef.current.set(
        eventSnapshotKey,
        (eventSequenceRef.current.get(eventSnapshotKey) ?? 0) + 1
      )
      setSnapshots((current) => ({
        ...current,
        [eventSnapshotKey]: snapshot
      }))
    },
    [enabled]
  )

  return { activeSnapshot, recordSnapshot }
}
