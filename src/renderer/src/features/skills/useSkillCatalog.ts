import { useCallback, useEffect, useRef, useState } from 'react'
import type { SkillsListOutput } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { getUserFacingErrorMessage } from '../../errors/userFacingError'
import { listSkills } from './skillsClient'

export type SkillCatalogState =
  | { status: 'idle' }
  | { status: 'loading'; projectId: string | null }
  | { status: 'ready'; projectId: string | null; output: SkillsListOutput }
  | { status: 'error'; projectId: string | null; message: string }

export function useSkillCatalog(projectId: string | null, enabled: boolean, refreshKey: string) {
  const { t } = useFrontendConfig()
  const requestSequenceRef = useRef(0)
  const [refreshSequence, setRefreshSequence] = useState(0)
  const [state, setState] = useState<SkillCatalogState>({ status: 'idle' })

  const refresh = useCallback(() => {
    setRefreshSequence((sequence) => sequence + 1)
  }, [])

  useEffect(() => {
    if (!enabled) {
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
          message: getUserFacingErrorMessage(error, t, 'chat.skillsLoadFailed')
        })
      })

    return () => {
      cancelled = true
    }
  }, [enabled, projectId, refreshKey, refreshSequence, t])

  return { refresh, state }
}
