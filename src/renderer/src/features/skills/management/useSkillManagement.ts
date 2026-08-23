// Renderer skills management state: serializes refreshes and owns revision-safe row mutations.
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction
} from 'react'
import type {
  SkillManagementEntry,
  SkillMutationOutput,
  SkillsListManagementOutput,
  SkillsSetEnabledOutput
} from '@mycopilot/protocol'
import type { TranslationKey } from '../../../config/languageRegistry'
import {
  getSkillOperationErrorDetails,
  getSkillOperationErrorKey,
  shouldRefreshSkillsAfterError
} from './skillManagementErrors'
import {
  listManagedSkills,
  onManagedSkillsChanged,
  setManagedSkillEnabled,
  uninstallManagedSkill
} from './skillsManagementClient'

export interface SkillManagementViewState {
  errorKey: TranslationKey | null
  isRefreshing: boolean
  output: SkillsListManagementOutput | null
  status: 'loading' | 'ready' | 'error'
}

export type SkillRowPendingOperation = 'enablement' | 'uninstall' | 'update'

const INITIAL_STATE: SkillManagementViewState = {
  errorKey: null,
  isRefreshing: false,
  output: null,
  status: 'loading'
}

export function useSkillManagement() {
  const [state, setState] = useState<SkillManagementViewState>(INITIAL_STATE)
  const [pendingOperations, setPendingOperations] = useState<
    ReadonlyMap<string, SkillRowPendingOperation>
  >(() => new Map())
  const mountedRef = useRef(false)
  const outputRef = useRef<SkillsListManagementOutput | null>(null)
  const requestedRefreshRef = useRef(0)
  const completedRefreshRef = useRef(0)
  const mutationEpochRef = useRef(0)
  const refreshLoopRef = useRef<Promise<void> | null>(null)

  const runRefreshLoop = useCallback(async () => {
    while (mountedRef.current && completedRefreshRef.current < requestedRefreshRef.current) {
      const requestNumber = requestedRefreshRef.current
      const mutationEpoch = mutationEpochRef.current
      setState((current) => ({
        ...current,
        errorKey: current.output ? current.errorKey : null,
        isRefreshing: Boolean(current.output),
        status: current.output ? 'ready' : 'loading'
      }))

      try {
        const output = filterGlobalManagementEntries(await listManagedSkills())
        completedRefreshRef.current = requestNumber
        if (
          !mountedRef.current ||
          requestNumber !== requestedRefreshRef.current ||
          mutationEpoch !== mutationEpochRef.current
        ) {
          continue
        }
        outputRef.current = output
        setState({ errorKey: null, isRefreshing: false, output, status: 'ready' })
      } catch (error) {
        completedRefreshRef.current = requestNumber
        if (
          !mountedRef.current ||
          requestNumber !== requestedRefreshRef.current ||
          mutationEpoch !== mutationEpochRef.current
        ) {
          continue
        }
        const errorKey = getSkillOperationErrorKey(
          getSkillOperationErrorDetails(error),
          'skills.loadFailed'
        )
        setState((current) =>
          current.output
            ? { ...current, errorKey, isRefreshing: false, status: 'ready' }
            : { errorKey, isRefreshing: false, output: null, status: 'error' }
        )
      }
    }
  }, [])

  const refresh = useCallback(async (): Promise<SkillsListManagementOutput | null> => {
    requestedRefreshRef.current += 1
    if (!refreshLoopRef.current) {
      const loop = runRefreshLoop().finally(() => {
        if (refreshLoopRef.current === loop) refreshLoopRef.current = null
      })
      refreshLoopRef.current = loop
    }
    await refreshLoopRef.current
    return outputRef.current
  }, [runRefreshLoop])

  useEffect(() => {
    mountedRef.current = true
    void refresh()
    const unsubscribe = onManagedSkillsChanged(() => {
      void refresh()
    })

    return () => {
      mountedRef.current = false
      requestedRefreshRef.current += 1
      unsubscribe()
    }
  }, [refresh])

  const setEnabled = useCallback(
    async (entry: SkillManagementEntry, enabled: boolean): Promise<SkillsSetEnabledOutput> => {
      mutationEpochRef.current += 1
      setPendingOperation(setPendingOperations, entry.id, 'enablement')
      try {
        const output = await setManagedSkillEnabled({
          enabled,
          expectedStateRevision: entry.stateRevision,
          skillId: entry.id
        })
        if (mountedRef.current) {
          patchEnablementOutput(outputRef, setState, output)
        }
        return output
      } catch (error) {
        const details = getSkillOperationErrorDetails(error)
        if (shouldRefreshSkillsAfterError(details)) await refresh()
        throw error
      } finally {
        if (mountedRef.current) clearPendingOperation(setPendingOperations, entry.id)
      }
    },
    [refresh]
  )

  const uninstall = useCallback(
    async (entry: SkillManagementEntry): Promise<SkillMutationOutput> => {
      if (!entry.installationRevision) {
        throw new Error('Managed Skill is missing its installation revision')
      }
      mutationEpochRef.current += 1
      setPendingOperation(setPendingOperations, entry.id, 'uninstall')
      try {
        const output = await uninstallManagedSkill({
          expectedRevision: entry.installationRevision,
          skillId: entry.id
        })
        if (mountedRef.current) removeManagementEntry(outputRef, setState, entry.id)
        void refresh()
        return output
      } catch (error) {
        const details = getSkillOperationErrorDetails(error)
        if (shouldRefreshSkillsAfterError(details)) await refresh()
        throw error
      } finally {
        if (mountedRef.current) clearPendingOperation(setPendingOperations, entry.id)
      }
    },
    [refresh]
  )

  return { pendingOperations, refresh, setEnabled, state, uninstall }
}

function filterGlobalManagementEntries(
  output: SkillsListManagementOutput
): SkillsListManagementOutput {
  return {
    ...output,
    skills: output.skills.filter((entry) => entry.source.kind !== 'workspace')
  }
}

function setPendingOperation(
  setPendingOperations: Dispatch<SetStateAction<ReadonlyMap<string, SkillRowPendingOperation>>>,
  skillId: string,
  operation: SkillRowPendingOperation
): void {
  setPendingOperations((current) => new Map(current).set(skillId, operation))
}

function clearPendingOperation(
  setPendingOperations: Dispatch<SetStateAction<ReadonlyMap<string, SkillRowPendingOperation>>>,
  skillId: string
): void {
  setPendingOperations((current) => {
    const next = new Map(current)
    next.delete(skillId)
    return next
  })
}

function patchEnablementOutput(
  outputRef: MutableRefObject<SkillsListManagementOutput | null>,
  setState: Dispatch<SetStateAction<SkillManagementViewState>>,
  mutation: SkillsSetEnabledOutput
): void {
  const current = outputRef.current
  if (!current) return
  const output = {
    ...current,
    managementRevision: mutation.managementRevision,
    skills: current.skills.map((entry) =>
      entry.id === mutation.skillId
        ? { ...entry, enabled: mutation.enabled, stateRevision: mutation.stateRevision }
        : entry
    )
  }
  outputRef.current = output
  setState({ errorKey: null, isRefreshing: false, output, status: 'ready' })
}

function removeManagementEntry(
  outputRef: MutableRefObject<SkillsListManagementOutput | null>,
  setState: Dispatch<SetStateAction<SkillManagementViewState>>,
  skillId: string
): void {
  const current = outputRef.current
  if (!current) return
  const output = { ...current, skills: current.skills.filter((entry) => entry.id !== skillId) }
  outputRef.current = output
  setState({ errorKey: null, isRefreshing: false, output, status: 'ready' })
}
