import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore
} from 'react'
import type { HumanInteractionHostApi } from '@mycopilot/host-api'
import type { HumanInteractionAnswer } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import {
  HumanInteractionController,
  type HumanInteractionControllerSnapshot
} from './humanInteractionController'
import {
  EMPTY_HUMAN_INTERACTION_DRAFT,
  humanInteractionAnswers,
  humanInteractionResponseDisplay,
  selectHumanInteractionRequest
} from './humanInteractionState'

export interface UseHumanInteractionOptions {
  conversationId: string | null
  hasApproval: boolean
  readOnly?: boolean
  /** Injectable only at the Renderer composition boundary; production uses the real Host API. */
  api?: HumanInteractionHostApi
}
const controllers = new WeakMap<HumanInteractionHostApi, HumanInteractionController>()
const emptySnapshot: HumanInteractionControllerSnapshot = {
  requests: {},
  drafts: {},
  operations: {},
  selected: {},
  minimized: {},
  loads: {}
}
const emptySubscribe = (): (() => void) => () => {}
const getEmptySnapshot = (): HumanInteractionControllerSnapshot => emptySnapshot

export function useHumanInteraction({
  conversationId,
  hasApproval,
  readOnly = false,
  api = hostClient.humanInteraction
}: UseHumanInteractionOptions) {
  const { t } = useFrontendConfig()
  const controller = useMemo(() => {
    if (!api) return null
    let existing = controllers.get(api)
    if (!existing) {
      existing = new HumanInteractionController(api)
      controllers.set(api, existing)
    }
    return existing
  }, [api])
  const state = useSyncExternalStore(
    controller?.subscribe ?? emptySubscribe,
    controller?.getSnapshot ?? getEmptySnapshot
  )
  const [refreshedScope, setRefreshedScope] = useState<{
    controller: HumanInteractionController
    conversationId: string
  } | null>(null)
  const approvalRefreshed =
    refreshedScope?.controller === controller && refreshedScope?.conversationId === conversationId
  const mountedRef = useRef(false)
  useLayoutEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])
  const accessRef = useRef({ controller, conversationId, hasApproval, readOnly, approvalRefreshed })
  useLayoutEffect(() => {
    accessRef.current = { controller, conversationId, hasApproval, readOnly, approvalRefreshed }
  }, [controller, conversationId, hasApproval, readOnly, approvalRefreshed])
  useEffect(() => controller?.connect(), [controller])
  const refresh = useCallback(async () => {
    if (controller && conversationId && !readOnly) {
      const succeeded = await controller.refresh(conversationId)
      const access = accessRef.current
      if (
        mountedRef.current &&
        succeeded &&
        access.controller === controller &&
        access.conversationId === conversationId &&
        !access.hasApproval &&
        !access.readOnly
      )
        setRefreshedScope({ controller, conversationId })
    }
  }, [controller, conversationId, readOnly])
  useEffect(() => {
    setRefreshedScope(null)
    if (!hasApproval) void refresh()
  }, [hasApproval, refresh])
  useEffect(() => {
    const refreshVisible = () => {
      if (document.visibilityState !== 'hidden') void refresh()
    }
    const unsubscribeResync = api?.onResync(() => void refresh())
    window.addEventListener('focus', refreshVisible)
    window.addEventListener('online', refreshVisible)
    document.addEventListener('visibilitychange', refreshVisible)
    return () => {
      unsubscribeResync?.()
      window.removeEventListener('focus', refreshVisible)
      window.removeEventListener('online', refreshVisible)
      document.removeEventListener('visibilitychange', refreshVisible)
    }
  }, [api, refresh])
  const requests = useMemo(
    () =>
      Object.values(state.requests)
        .filter((request) => request.conversationId === conversationId)
        .sort((a, b) => b.sequence - a.sequence),
    [state.requests, conversationId]
  )
  const openRequests = requests.filter((request) => request.status === 'open')
  const blockingBatch = openRequests.find((request) => request.mode === 'sync') ?? null
  const selectedBatch = selectHumanInteractionRequest(
    requests,
    conversationId ? state.selected[conversationId] : null,
    state.minimized
  )
  const activeBatch = readOnly || hasApproval || !approvalRefreshed ? null : selectedBatch
  const activeDraft = activeBatch
    ? (state.drafts[activeBatch.requestId] ?? EMPTY_HUMAN_INTERACTION_DRAFT)
    : EMPTY_HUMAN_INTERACTION_DRAFT
  const operation = activeBatch ? state.operations[activeBatch.requestId] : null
  const canAccess = useCallback(
    (requestId: string, requireActive = true): boolean => {
      const current = controller?.getSnapshot()
      const access = accessRef.current
      const request = current?.requests[requestId]
      const blockedBySync =
        request?.mode === 'async' &&
        Object.values(current?.requests ?? {}).some(
          (candidate) =>
            candidate.conversationId === request.conversationId &&
            candidate.mode === 'sync' &&
            candidate.status === 'open'
        )
      const selected =
        current &&
        selectHumanInteractionRequest(
          Object.values(current.requests).filter(
            (candidate) => candidate.conversationId === access.conversationId
          ),
          access.conversationId ? current.selected[access.conversationId] : null,
          current.minimized
        )
      return Boolean(
        (!requireActive || selected?.requestId === requestId) &&
        mountedRef.current &&
        controller &&
        access.controller === controller &&
        access.conversationId &&
        !access.readOnly &&
        !access.hasApproval &&
        access.approvalRefreshed &&
        request?.conversationId === access.conversationId &&
        request.status === 'open' &&
        !blockedBySync
      )
    },
    [controller]
  )
  const setPage = useCallback(
    (requestId: string, index: number) => {
      if (canAccess(requestId)) controller?.setPage(requestId, index)
    },
    [canAccess, controller]
  )
  const setAnswer = useCallback(
    (requestId: string, answer: HumanInteractionAnswer) => {
      if (canAccess(requestId)) controller?.setAnswer(requestId, answer)
    },
    [canAccess, controller]
  )
  const submit = useCallback(
    async (requestId: string) => {
      if (canAccess(requestId)) await controller?.submit(requestId)
    },
    [canAccess, controller]
  )
  const ignore = useCallback(
    async (requestId: string) => {
      if (canAccess(requestId)) await controller?.ignore(requestId)
    },
    [canAccess, controller]
  )
  const open = useCallback(
    (requestId: string) => {
      if (canAccess(requestId, false) && !blockingBatch) controller?.open(requestId)
    },
    [canAccess, controller, blockingBatch]
  )
  const minimize = useCallback(
    (requestId: string) => {
      if (canAccess(requestId)) controller?.minimize(requestId)
    },
    [canAccess, controller]
  )
  const historyResponses = useMemo(
    () =>
      requests.flatMap((request) => {
        const display = humanInteractionResponseDisplay(request)
        return display ? [{ request, display }] : []
      }),
    [requests]
  )
  const errorCode = operation?.error ?? (conversationId ? state.loads[conversationId]?.error : null)
  const error =
    errorCode === 'outcome_unknown'
      ? t('humanInteraction.error.outcomeUnknown')
      : errorCode === 'state_changed'
        ? t('humanInteraction.error.stateChanged')
        : errorCode
          ? t('humanInteraction.error.loadFailed')
          : null
  return {
    requests,
    openRequests,
    activeBatch,
    activeDraft,
    draftsByRequestId: state.drafts,
    historyResponses,
    status: conversationId ? (state.loads[conversationId]?.status ?? 'loading') : 'ready',
    error,
    isSubmitting: operation?.isSubmitting ?? false,
    isDraftLocked: operation?.isDraftLocked ?? false,
    pendingAction: operation?.pendingAction ?? null,
    canSubmit: Boolean(
      activeBatch &&
      !operation?.isSubmitting &&
      operation?.pendingAction !== 'ignore' &&
      humanInteractionAnswers(activeBatch, activeDraft)
    ),
    canInteract: !readOnly && !hasApproval && approvalRefreshed,
    refresh,
    setPage,
    setAnswer,
    submit,
    ignore,
    open,
    minimize
  }
}
export type HumanInteractionControllerView = ReturnType<typeof useHumanInteraction>
