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
  humanInteractionResponseDisplay
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
  const [approvalRefreshed, setApprovalRefreshed] = useState(false)
  const mountedRef = useRef(false)
  useLayoutEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])
  const accessRef = useRef({ conversationId, hasApproval, readOnly, approvalRefreshed })
  useLayoutEffect(() => {
    accessRef.current = { conversationId, hasApproval, readOnly, approvalRefreshed }
  }, [conversationId, hasApproval, readOnly, approvalRefreshed])
  useEffect(() => controller?.connect(), [controller])
  const refresh = useCallback(async () => {
    if (controller && conversationId && !readOnly) {
      const succeeded = await controller.refresh(conversationId)
      const access = accessRef.current
      if (
        mountedRef.current &&
        succeeded &&
        access.conversationId === conversationId &&
        !access.hasApproval &&
        !access.readOnly
      )
        setApprovalRefreshed(true)
    }
  }, [controller, conversationId, readOnly])
  useEffect(() => {
    setApprovalRefreshed(false)
    if (!hasApproval) void refresh()
  }, [hasApproval, refresh])
  useEffect(() => {
    const refreshVisible = () => {
      if (document.visibilityState !== 'hidden') void refresh()
    }
    window.addEventListener('focus', refreshVisible)
    window.addEventListener('online', refreshVisible)
    document.addEventListener('visibilitychange', refreshVisible)
    return () => {
      window.removeEventListener('focus', refreshVisible)
      window.removeEventListener('online', refreshVisible)
      document.removeEventListener('visibilitychange', refreshVisible)
    }
  }, [refresh])
  const requests = useMemo(
    () =>
      Object.values(state.requests)
        .filter((request) => request.conversationId === conversationId)
        .sort((a, b) => b.sequence - a.sequence),
    [state.requests, conversationId]
  )
  const openRequests = requests.filter((request) => request.status === 'open')
  const blockingBatch = openRequests.find((request) => request.mode === 'sync') ?? null
  const selectedId = conversationId ? state.selected[conversationId] : null
  const selected = openRequests.find((request) => request.requestId === selectedId)
  const asyncBatch = selected
    ? state.minimized[selected.requestId]
      ? null
      : selected
    : (openRequests.find(
        (request) => request.mode === 'async' && !state.minimized[request.requestId]
      ) ?? null)
  const activeBatch =
    readOnly || hasApproval || !approvalRefreshed ? null : (blockingBatch ?? asyncBatch)
  const activeDraft = activeBatch
    ? (state.drafts[activeBatch.requestId] ?? EMPTY_HUMAN_INTERACTION_DRAFT)
    : EMPTY_HUMAN_INTERACTION_DRAFT
  const operation = activeBatch ? state.operations[activeBatch.requestId] : null
  const canAccess = useCallback(
    (requestId: string): boolean => {
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
      return Boolean(
        mountedRef.current &&
        controller &&
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
      if (canAccess(requestId) && !blockingBatch) controller?.open(requestId)
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
