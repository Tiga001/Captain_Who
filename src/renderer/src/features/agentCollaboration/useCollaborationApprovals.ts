import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  CollaborationApprovalDecisionResult,
  CollaborationApprovalProjection,
  CollaborationApprovalStatus
} from '@mycopilot/protocol'
import { decideCollaborationApproval, listCollaborationApprovals } from './collaborationClient'
import type { CollaborationApprovalDecisionHandler } from './ProjectedApprovalDecisionCard'

export interface UseCollaborationApprovalsOptions {
  enabled?: boolean
  /** Root-local durable sequence supplied by the shared collaboration store. */
  invalidationSequence: number
  rootConversationId: string | null
}

export interface CollaborationApprovalsController {
  approvals: readonly CollaborationApprovalProjection[]
  decide: CollaborationApprovalDecisionHandler
  error: string | null
  loading: boolean
  refresh: () => Promise<void>
}

/**
 * Approval projection controller with no subscription of its own. The AppShell supplies the
 * shared root-store sequence, so chat activity and Agent Center do not create parallel listeners.
 */
export function useCollaborationApprovals({
  enabled = true,
  invalidationSequence,
  rootConversationId
}: UseCollaborationApprovalsOptions): CollaborationApprovalsController {
  const [approvals, setApprovals] = useState<readonly CollaborationApprovalProjection[]>([])
  const [approvalsRootConversationId, setApprovalsRootConversationId] = useState<string | null>(
    rootConversationId
  )
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const generationRef = useRef(0)
  const inFlightRef = useRef(new Set<string>())
  const decisionStatusesRef = useRef(new Map<string, CollaborationApprovalStatus>())
  const rootConversationIdRef = useRef(rootConversationId)

  useEffect(() => {
    rootConversationIdRef.current = rootConversationId
  }, [rootConversationId])

  const applyAuthoritativeDecisionStatus = useCallback(
    (decisionRootConversationId: string, result: CollaborationApprovalDecisionResult) => {
      if (result.status === 'pending') return

      // Only decision acknowledgements are retained here. List-only states may legitimately
      // change during recovery; an old pending list must not reopen an acknowledged decision.
      decisionStatusesRef.current.set(
        approvalScopeKey(decisionRootConversationId, result.approvalId),
        result.status
      )
      if (rootConversationIdRef.current !== decisionRootConversationId) return

      setApprovals((current) =>
        current.map((approval) =>
          approval.approvalId === result.approvalId && approval.status === 'pending'
            ? { ...approval, status: result.status }
            : approval
        )
      )
    },
    []
  )

  const refresh = useCallback(async () => {
    // A decision may finish after navigation and still hold this root's refresh callback.
    // Ignore it before touching either the active scope or its request generation.
    if (rootConversationIdRef.current !== rootConversationId) return
    const generation = ++generationRef.current
    setApprovalsRootConversationId((currentRootConversationId) => {
      if (currentRootConversationId !== rootConversationId) {
        setApprovals([])
        setError(null)
      }
      return rootConversationId
    })
    if (!enabled || !rootConversationId) {
      setApprovals([])
      setError(null)
      setLoading(false)
      return
    }

    setLoading(true)
    try {
      const projection = await listCollaborationApprovals({ rootConversationId })
      if (
        generationRef.current !== generation ||
        rootConversationIdRef.current !== rootConversationId
      ) {
        return
      }
      setApprovals(
        projection.approvals.map((approval) => {
          const acknowledgedStatus = decisionStatusesRef.current.get(
            approvalScopeKey(rootConversationId, approval.approvalId)
          )
          return approval.status === 'pending' && acknowledgedStatus
            ? { ...approval, status: acknowledgedStatus }
            : approval
        })
      )
      setApprovalsRootConversationId(rootConversationId)
      setError(null)
    } catch (loadError) {
      if (generationRef.current !== generation) return
      setError(loadError instanceof Error ? loadError.message : String(loadError))
    } finally {
      if (generationRef.current === generation) setLoading(false)
    }
  }, [enabled, rootConversationId])

  useEffect(() => {
    void refresh()
    return () => {
      generationRef.current += 1
    }
  }, [invalidationSequence, refresh])

  const decide = useCallback<CollaborationApprovalDecisionHandler>(
    async (approvalId, decision, message) => {
      if (!enabled || !rootConversationId) {
        throw new Error('Collaboration approval is unavailable.')
      }
      const scopeKey = approvalScopeKey(rootConversationId, approvalId)
      if (inFlightRef.current.has(scopeKey)) {
        throw new Error('Collaboration approval decision is already in progress.')
      }

      inFlightRef.current.add(scopeKey)
      try {
        const decisionRootConversationId = rootConversationId
        const result = await decideCollaborationApproval({
          approvalId,
          decision,
          message,
          rootConversationId: decisionRootConversationId
        })

        // The decision response is already an authoritative durable acknowledgement. Reflect a
        // non-pending status immediately, then use the existing projection reload to close any
        // notification gap. A failed reload must not regress an accepted decision back to a
        // clickable pending card.
        applyAuthoritativeDecisionStatus(decisionRootConversationId, result)
        await refresh()
        applyAuthoritativeDecisionStatus(decisionRootConversationId, result)
        return result
      } finally {
        inFlightRef.current.delete(scopeKey)
      }
    },
    [applyAuthoritativeDecisionStatus, enabled, refresh, rootConversationId]
  )

  const isCurrentScope = approvalsRootConversationId === rootConversationId
  return {
    approvals: isCurrentScope ? approvals : [],
    decide,
    error: isCurrentScope ? error : null,
    loading: isCurrentScope ? loading : Boolean(enabled && rootConversationId),
    refresh
  }
}

function approvalScopeKey(rootConversationId: string, approvalId: string): string {
  return JSON.stringify([rootConversationId, approvalId])
}
