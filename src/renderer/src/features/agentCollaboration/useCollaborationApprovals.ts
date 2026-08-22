import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  CollaborationApprovalDecisionResult,
  CollaborationApprovalProjection
} from '@mycopilot/protocol'
import { decideCollaborationApproval, listCollaborationApprovals } from './collaborationClient'
import type { CollaborationApprovalDecision } from './CollaborationApprovalPanel'

export interface UseCollaborationApprovalsOptions {
  enabled?: boolean
  /** Root-local durable sequence supplied by the shared collaboration store. */
  invalidationSequence: number
  rootConversationId: string | null
}

export interface CollaborationApprovalsController {
  approvals: readonly CollaborationApprovalProjection[]
  decide: (
    approvalId: string,
    decision: CollaborationApprovalDecision,
    message: string | null
  ) => Promise<CollaborationApprovalDecisionResult>
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
  const rootConversationIdRef = useRef(rootConversationId)

  useEffect(() => {
    rootConversationIdRef.current = rootConversationId
  }, [rootConversationId])

  const applyAuthoritativeDecisionStatus = useCallback(
    (decisionRootConversationId: string, result: CollaborationApprovalDecisionResult) => {
      if (
        result.status === 'pending' ||
        rootConversationIdRef.current !== decisionRootConversationId
      ) {
        return
      }

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
      if (generationRef.current !== generation) return
      setApprovals(projection.approvals)
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

  const decide = useCallback(
    async (approvalId: string, decision: CollaborationApprovalDecision, message: string | null) => {
      if (!enabled || !rootConversationId) {
        throw new Error('Collaboration approval is unavailable.')
      }
      if (inFlightRef.current.has(approvalId)) {
        throw new Error('Collaboration approval decision is already in progress.')
      }

      inFlightRef.current.add(approvalId)
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
        inFlightRef.current.delete(approvalId)
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
