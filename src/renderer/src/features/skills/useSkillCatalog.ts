import { useCallback, useEffect, useRef, useState } from 'react'
import type { SkillsListOutput } from '@mycopilot/protocol'
import { listSkills } from './skillsClient'

export type SkillCatalogState =
  | { status: 'idle' }
  | { status: 'loading'; projectId: string }
  | { status: 'ready'; projectId: string; output: SkillsListOutput }
  | { status: 'error'; projectId: string; message: string }

export function useSkillCatalog(projectId: string | null, enabled: boolean, refreshKey: string) {
  const requestSequenceRef = useRef(0)
  const [refreshSequence, setRefreshSequence] = useState(0)
  const [state, setState] = useState<SkillCatalogState>({ status: 'idle' })

  const refresh = useCallback(() => {
    setRefreshSequence((sequence) => sequence + 1)
  }, [])

  useEffect(() => {
    if (!enabled || !projectId) {
      requestSequenceRef.current += 1
      setState({ status: 'idle' })
      return
    }

    const requestSequence = requestSequenceRef.current + 1
    requestSequenceRef.current = requestSequence
    let cancelled = false
    setState({ status: 'loading', projectId })

    void listSkills(projectId)
      .then((output) => {
        if (cancelled || requestSequenceRef.current !== requestSequence) return
        setState({ status: 'ready', projectId, output })
      })
      .catch((error) => {
        if (cancelled || requestSequenceRef.current !== requestSequence) return
        setState({
          status: 'error',
          projectId,
          message: error instanceof Error ? error.message : String(error)
        })
      })

    return () => {
      cancelled = true
    }
  }, [enabled, projectId, refreshKey, refreshSequence])

  return { refresh, state }
}
