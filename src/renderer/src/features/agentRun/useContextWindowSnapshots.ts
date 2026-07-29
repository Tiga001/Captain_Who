import { useCallback, useEffect, useRef, useState } from 'react'
import type { AgentContextWindowSnapshot, SkillSelection } from '@mycopilot/protocol'
import { getContextWindowSnapshot } from '../agent/agentClient'
import { resolveChatPermissions } from '../chat/chatPermissions'
import type { ChatPermissionMode } from '../chat/chatTypes'
import type { UiPreferencesSnapshot } from '../storage/storageClient'
import { DEFAULT_AGENT_MAX_TOKENS } from './constants'

function snapshotKey(scopeId: string, modelId: string): string {
  return `${scopeId}\u0000${modelId}`
}

interface UseContextWindowSnapshotsOptions {
  conversationId?: string
  customPermissions: UiPreferencesSnapshot['customPermissions']
  enabled: boolean
  modelId: string | null
  permissionMode: ChatPermissionMode
  projectId: string | null
  scopeId: string
  skills: SkillSelection[]
}

export function useContextWindowSnapshots({
  conversationId,
  customPermissions,
  enabled,
  modelId,
  permissionMode,
  projectId,
  scopeId,
  skills
}: UseContextWindowSnapshotsOptions) {
  const requestSequenceRef = useRef(0)
  const eventSequenceRef = useRef<Map<string, number>>(new Map())
  const [snapshots, setSnapshots] = useState<Record<string, AgentContextWindowSnapshot>>({})
  const activeSnapshotKey = modelId ? snapshotKey(scopeId, modelId) : null
  const activeSnapshot = enabled && activeSnapshotKey ? snapshots[activeSnapshotKey] : undefined

  useEffect(() => {
    if (!enabled || !modelId) return undefined

    const requestSequence = requestSequenceRef.current + 1
    requestSequenceRef.current = requestSequence
    const requestedSnapshotKey = snapshotKey(scopeId, modelId)
    const eventSequenceAtRequest = eventSequenceRef.current.get(requestedSnapshotKey) ?? 0
    let cancelled = false

    void getContextWindowSnapshot({
      conversationId,
      projectId,
      modelId,
      maxTokens: DEFAULT_AGENT_MAX_TOKENS,
      skills: skills.length > 0 ? skills : undefined,
      permissions: resolveChatPermissions(permissionMode, customPermissions)
    })
      .then(({ snapshot }) => {
        if (cancelled || requestSequenceRef.current !== requestSequence) return
        if ((eventSequenceRef.current.get(requestedSnapshotKey) ?? 0) !== eventSequenceAtRequest) {
          return
        }
        setSnapshots((current) => {
          if (snapshot?.model === modelId) {
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
    conversationId,
    customPermissions,
    enabled,
    modelId,
    permissionMode,
    projectId,
    scopeId,
    skills
  ])

  const recordSnapshot = useCallback(
    (eventScopeId: string, snapshot: AgentContextWindowSnapshot) => {
      if (!enabled) return
      const eventSnapshotKey = snapshotKey(eventScopeId, snapshot.model)
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
