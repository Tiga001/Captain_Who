// Isolated acceptance shell: all submission, persistence, lifecycle and control hooks are production.
import { useCallback, useEffect, useRef, useState, type SetStateAction } from 'react'
import { createRoot } from 'react-dom/client'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { AgentRunLifecycleRefs } from '../agentRunLifecycleSupport'
import type { ChatComposerDraft, ChatConversation } from '../../features/chat/chatTypes'
import { getAgentActionApprovalStatus } from '../../features/agentRun/agentActionUtils'
import type {
  AcceptanceAction,
  AcceptanceControls,
  AcceptanceResult,
  AcceptanceWork
} from '../../../../main/core/fixtures/backendAcceptance.types'

declare global {
  interface Window {
    acceptanceTransport: {
      onAgentRpcSettled: (
        callback: (result: { channel: string; ok: boolean }) => void
      ) => () => void
    }
    acceptanceControls?: AcceptanceControls
  }
}

const { FrontendConfigProvider, useFrontendConfig } =
  await import('../../config/FrontendConfigProvider')
const { loadModelSettings, loadUiPreferences, loadProjects } =
  await import('../../features/storage/storageClient')
const { useAppShellMessageSubmission } = await import('../useAppShellMessageSubmission')
const { useAgentRunLifecycle } = await import('../useAgentRunLifecycle')
const { useConversationPersistence } = await import('../useConversationPersistence')
const { createComposerDraft } = await import('../chatMessageFactory')
const { useAgentActionDecisionHandlers } =
  await import('../../features/agentRun/useAgentActionDecisionHandlers')
const { useAppShellRunControls } = await import('../useAppShellRunControls')
const { actionDiagnostics } = await import('../../diagnostics/actionDiagnostics')
const diagnosticWindows: object[] = []
Reflect.set(actionDiagnostics, 'emit', (summary: object) => diagnosticWindows.push(summary))
const settings = await loadModelSettings()
if (!settings?.models[0]) throw new Error('Missing real Core fixture model')
const model = settings.models[0]
const uiPreferences = await loadUiPreferences()
const project = (await loadProjects()).find((item) => item.id === 'acceptance-project')
if (!project) throw new Error('Missing isolated fixture project')
const projectId = project.id
type SubmissionOptions = Parameters<typeof useAppShellMessageSubmission>[0]

function createLifecycleRefs(): AgentRunLifecycleRefs {
  return {
    activeRunBindings: { current: new Map() },
    autoSubmitQueuedMessage: { current: () => undefined },
    bufferedAgentEvents: { current: new Map() },
    cancelledPendingMessageIds: { current: new Set() },
    cancelledRunIds: { current: new Set() },
    locallyUnconfirmedStoppedRunIds: { current: new Set() },
    pendingActionsHydrated: { current: new Set() },
    pendingGuidancePayloads: { current: new Map() },
    pendingMessageDeltas: { current: new Map() },
    retiredAgentRunIds: { current: new Set() },
    stopReconciliationTimers: { current: new Map() },
    stopRequestedPendingMessageIds: { current: new Set() },
    stopRequestedRunIds: { current: new Set() }
  }
}

function Workspace() {
  const { t } = useFrontendConfig()
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null)
  const activeConversationIdRef = useRef<string | null>(null)
  const [conversations, setConversations] = useState<ChatConversation[]>([])
  const conversationsRef = useRef(conversations)
  const [drafts, setDrafts] = useState<Record<string, ChatComposerDraft>>({
    new: createComposerDraft({ modelId: model.id })
  })
  const draftsRef = useRef(drafts)
  const [refs] = useState(createLifecycleRefs)
  const [notices, setNotices] = useState<string[]>([])
  const showToast = useCallback(
    (notice: string) => setNotices((current) => [...current, notice]),
    []
  )
  const editRewriteAttemptsRef = useRef<SubmissionOptions['editRewriteAttemptsRef']['current']>(
    new Map()
  )
  const editRewriteInFlightRef = useRef(new Set<string>())
  const editSubmissionSeqRef = useRef(0)
  const pendingProviderTransitionSubmissionsRef = useRef<
    SubmissionOptions['pendingProviderTransitionSubmissionsRef']['current']
  >(new Map())
  const persistence = useConversationPersistence()
  const setConversationsWithRef = useCallback((value: SetStateAction<ChatConversation[]>) => {
    const next = typeof value === 'function' ? value(conversationsRef.current) : value
    conversationsRef.current = next
    setConversations(next)
  }, [])
  const updateDraft = useCallback((id: string, draft: ChatComposerDraft) => {
    const next = { ...draftsRef.current, [id]: draft }
    draftsRef.current = next
    setDrafts(next)
  }, [])
  const mutateDraft = useCallback(
    (id: string, updater: (draft: ChatComposerDraft) => ChatComposerDraft) => {
      const next = updater(draftsRef.current[id] ?? createComposerDraft({ modelId: model.id }))
      updateDraft(id, next)
      return next
    },
    [updateDraft]
  )
  const lifecycle = useAgentRunLifecycle({
    contextWindowIndicatorEnabled: false,
    conversationState: {
      activeConversationId,
      activeConversationIdRef,
      conversations,
      conversationsRef,
      setActiveConversationId,
      setConversations: setConversationsWithRef
    },
    draftState: { draftsRef, mutateDraft },
    enqueueConversationMetaSave: persistence.enqueueConversationMetaSave,
    enqueueChatMessageCheckpoint: persistence.enqueueChatMessageCheckpoint,
    enqueueChatMessageStateSave: persistence.enqueueChatMessageStateSave,
    flushChatMessageStateSave: persistence.flushChatMessageStateSave,
    flushConversationMessageStateSaves: persistence.flushConversationMessageStateSaves,
    recordContextWindowSnapshot: () => undefined,
    reconcileFailedSkillActivation: () => undefined,
    refs,
    requestSkillCatalogRefresh: () => undefined,
    sealAndFlushChatMessageStateSaves: persistence.sealAndFlushChatMessageStateSaves,
    showToast,
    t,
    uiPreferences
  })
  const submission = useAppShellMessageSubmission({
    activeConversationIdRef,
    activeDraft: drafts[activeConversationId ?? 'new'],
    activeDraftSelectedModel: model,
    autoSubmitQueuedMessageRef: refs.autoSubmitQueuedMessage,
    conversations,
    conversationsRef,
    drafts,
    draftsRef,
    editRewriteAttemptsRef,
    editRewriteInFlightRef,
    editSubmissionSeqRef,
    enabledModels: settings!.models,
    enqueueChatMessagesUpsert: persistence.enqueueChatMessagesUpsert,
    enqueueConversationMetaSave: persistence.enqueueConversationMetaSave,
    mutateDraft,
    pendingProviderTransitionSubmissionsRef,
    requestAssistantResponse: lifecycle.requestAssistantResponse,
    restoreSubmittedSkills: () => undefined,
    setActiveConversationId,
    setActiveConversationInitialScrollTop: () => undefined,
    setConversationScrollToBottomSignal: () => undefined,
    setConversationsWithRef,
    setScrollTargetMessageId: () => undefined,
    showToast,
    t,
    updateDraft,
    waitForConversationSaves: persistence.waitForConversationSaves,
    waitForMessageUpserts: persistence.waitForMessageUpserts,
    waitForMessageStateSaves: persistence.waitForMessageStateSaves,
    waitForRunSettlement: lifecycle.waitForRunSettlement
  })
  const decisions = useAgentActionDecisionHandlers({
    activeConversationId,
    conversationsRef,
    flushMessagePersistence: lifecycle.flushRunMessagePersistence,
    updateAssistantMessage: lifecycle.updateAssistantMessage
  })
  const controls = useAppShellRunControls({
    activeConversationId,
    activeConversationIdRef,
    activeRunBindingsRef: refs.activeRunBindings,
    autoSubmitQueuedMessageRef: refs.autoSubmitQueuedMessage,
    conversations,
    conversationsRef,
    draftsRef,
    flushRunMessagePersistence: lifecycle.flushRunMessagePersistence,
    mutateDraft,
    pendingGuidancePayloadsRef: refs.pendingGuidancePayloads,
    removeQueuedMessageByClientId: lifecycle.removeQueuedMessageByClientId,
    restoreRejectedGuidance: lifecycle.restoreRejectedGuidance,
    supersedeRejectedGuidance: lifecycle.supersedeRejectedGuidance,
    scheduleStoppedRunReconciliation: lifecycle.scheduleStoppedRunReconciliation,
    stopRequestedPendingMessageIdsRef: refs.stopRequestedPendingMessageIds,
    stopRequestedRunIdsRef: refs.stopRequestedRunIds,
    t,
    updateAssistantMessage: lifecycle.updateAssistantMessage
  })
  const pendingClickRef = useRef<{
    action: AcceptanceAction
    work?: AcceptanceWork
    index?: number
    resolve: (result: AcceptanceResult) => void
    reject: (error: unknown) => void
  } | null>(null)
  const snapshot = useCallback(() => {
    const conversation = conversationsRef.current.find(
      (item) => item.id === activeConversationIdRef.current
    )
    const message = conversation?.messages.findLast((item) => item.role === 'assistant')
    return {
      conversationId: conversation?.id ?? null,
      assistantMessageId: message?.id ?? null,
      runId: message?.agentRun?.runId ?? null,
      status: message?.agentRun?.status ?? null,
      content: message?.content ?? '',
      pendingApprovals:
        message?.agentRun?.approvals.filter(
          (action) => getAgentActionApprovalStatus(action) === 'required'
        ).length ?? 0,
      renderedContent: document.getElementById('acceptance-output')?.textContent ?? '',
      renderedStatus: document.getElementById('acceptance-output')?.dataset.status || null,
      notices
    }
  }, [notices])
  const executeAction = async () => {
    const request = pendingClickRef.current
    if (!request) return
    pendingClickRef.current = null
    const started = performance.now()
    try {
      let accepted = true
      if (request.action === 'submit') {
        accepted = await submission.submitMessage(
          `Synthetic local acceptance. CAPTAIN_ACCEPTANCE:${request.work}:${request.index}`,
          {
            modelId: model.id,
            permissionMode: 'default',
            projectId,
            attachments: [],
            skills: []
          }
        )
      } else if (request.action === 'approve') {
        const current = snapshot()
        const message = conversationsRef.current
          .find((item) => item.id === current.conversationId)
          ?.messages.find((item) => item.id === current.assistantMessageId)
        const action = message?.agentRun?.approvals.find(
          (item) => getAgentActionApprovalStatus(item) === 'required'
        )
        if (!message || !action) throw new Error('No exact pending action')
        accepted = await decisions.handleApproveAgentAction(message.id, action, 'singleAction')
      } else {
        // The production Stop callback is intentionally fire-and-forget. Observe its actual IPC
        // settlement without changing its return or treating that receipt as a durable terminal.
        accepted = await new Promise<boolean>((resolve, reject) => {
          const timer = setTimeout(() => {
            unsubscribe()
            reject(new Error('Stop receipt timeout'))
          }, 15000)
          const unsubscribe = window.acceptanceTransport.onAgentRpcSettled((result) => {
            if (result.channel !== HOST_CHANNELS.agent.cancelRun) return
            clearTimeout(timer)
            unsubscribe()
            resolve(result.ok)
          })
          controls.stopActiveGeneration()
        })
      }
      const receiptMs = performance.now() - started
      if (!accepted) throw new Error('Production action did not accept the operation')
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
      )
      request.resolve({
        action: request.action,
        accepted,
        receiptMs,
        frameOpportunityMs: performance.now() - started,
        ...snapshot()
      })
    } catch (error) {
      request.reject(error)
    }
  }
  useEffect(() => {
    const click = (action: AcceptanceAction, work?: AcceptanceWork, index?: number) =>
      new Promise<AcceptanceResult>((resolve, reject) => {
        if (pendingClickRef.current) {
          reject(new Error('Concurrent fixture action'))
          return
        }
        pendingClickRef.current = { action, work, index, resolve, reject }
        document.getElementById(`acceptance-${action}`)!.click()
      })
    window.acceptanceControls = {
      submit: (work, index) => click('submit', work, index),
      approve: () => click('approve'),
      stop: () => click('stop'),
      snapshot,
      flushDiagnostics: () => {
        actionDiagnostics.flush()
        return diagnosticWindows.splice(0)
      }
    }
    return () => {
      delete window.acceptanceControls
    }
  }, [snapshot])
  const current = conversations
    .find((conversation) => conversation.id === activeConversationId)
    ?.messages.findLast((message) => message.role === 'assistant')
  return (
    <main>
      <button id="acceptance-submit" type="button" onClick={() => void executeAction()}>
        Send
      </button>
      <button id="acceptance-approve" type="button" onClick={() => void executeAction()}>
        Approve
      </button>
      <button id="acceptance-stop" type="button" onClick={() => void executeAction()}>
        Stop
      </button>
      <output id="acceptance-output" data-status={current?.agentRun?.status ?? ''}>
        {current?.content ?? ''}
      </output>
    </main>
  )
}
createRoot(document.getElementById('root')!).render(
  <FrontendConfigProvider>
    <Workspace />
  </FrontendConfigProvider>
)
