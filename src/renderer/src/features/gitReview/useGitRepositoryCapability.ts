import { useEffect, useRef, useState } from 'react'
import type { RightSidebarCapabilityState } from '../rightSidebar/rightSidebarTypes'
import { createRightSidebarWorkspaceSessionKey } from '../rightSidebar/rightSidebarWorkspace'
import { inspectGitRepository } from './gitReviewClient'

export function useGitRepositoryCapability(
  projectId: string | null | undefined,
  workspacePath: string | undefined,
  projectRevision = ''
): RightSidebarCapabilityState {
  const contextKey = createRightSidebarWorkspaceSessionKey(projectId, workspacePath)
  const hasWorkspace = Boolean(projectId && workspacePath)
  const [resolvedState, setResolvedState] = useState<RightSidebarCapabilityState | null>(null)
  const generationRef = useRef(0)

  useEffect(() => {
    const generation = generationRef.current + 1
    generationRef.current = generation

    if (!projectId || !workspacePath) {
      setResolvedState({ contextKey, status: 'unavailable' })
      return undefined
    }

    let cancelled = false
    setResolvedState({ contextKey, status: 'checking' })
    void inspectGitRepository(projectId)
      .then((inspection) => {
        if (cancelled || generationRef.current !== generation) return
        if (inspection.projectId !== projectId) {
          console.warn('Git repository inspection returned a different project identity')
          setResolvedState({ contextKey, status: 'unavailable' })
          return
        }

        const available = inspection.state === 'ready' && Boolean(inspection.repositoryId)
        setResolvedState({
          contextKey,
          identity: available ? inspection.repositoryId : undefined,
          status: available ? 'available' : 'unavailable'
        })
      })
      .catch((error) => {
        if (cancelled || generationRef.current !== generation) return
        console.warn('Failed to inspect Git repository capability', error)
        setResolvedState({ contextKey, status: 'unavailable' })
      })

    return () => {
      cancelled = true
    }
  }, [contextKey, projectId, workspacePath, projectRevision])

  if (resolvedState?.contextKey === contextKey) return resolvedState
  return {
    contextKey,
    status: hasWorkspace ? 'checking' : 'unavailable'
  }
}
