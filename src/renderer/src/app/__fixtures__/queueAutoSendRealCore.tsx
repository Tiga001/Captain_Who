// Only Electron transport and the shell's state owner are adapted. Submission, lifecycle,
// message persistence, Preload/Main, preflight and turn execution are production modules.
import { useCallback, useRef, useState, type SetStateAction } from 'react'
import { createRoot } from 'react-dom/client'
import type { IpcRenderer } from 'electron'
import type { HostApi } from '@mycopilot/host-api'
import { createAgentIpcBridge } from '../../../../preload/AgentIpcBridge'
import { createStorageIpcBridge } from '../../../../preload/StorageIpcBridge'
import type { AgentRunLifecycleRefs } from '../agentRunLifecycleSupport'
import type { ChatComposerDraft, ChatConversation } from '../../features/chat/chatTypes'
import '../../styles/global.css'
import '../../features/chat/components/GuidanceQueue.css'

declare global {
  interface Window {
    __queueInvoke: (channel: string, input: unknown) => Promise<unknown>
  }
}
const listeners = new Map<(...args: never[]) => unknown, EventListener>()
const ipc = {
  invoke: (channel: string, input: unknown) => window.__queueInvoke(channel, input),
  on: (channel: string, callback: (...args: never[]) => unknown) => {
    const listener: EventListener = (event) =>
      callback({} as never, (event as CustomEvent).detail as never)
    listeners.set(callback, listener)
    window.addEventListener(channel, listener)
  },
  removeListener: (channel: string, callback: (...args: never[]) => unknown) => {
    const listener = listeners.get(callback)
    if (listener) window.removeEventListener(channel, listener)
    listeners.delete(callback)
  }
} as unknown as IpcRenderer
window.mycopilot = {
  host: {
    agent: createAgentIpcBridge(ipc),
    storage: createStorageIpcBridge(ipc),
    app: { setNativeThemeSource: async () => undefined }
  } as unknown as HostApi
}
const { FrontendConfigProvider, useFrontendConfig } =
  await import('../../config/FrontendConfigProvider')
const { loadModelSettings, loadUiPreferences } =
  await import('../../features/storage/storageClient')
const { useAppShellMessageSubmission } = await import('../useAppShellMessageSubmission')
const { useAgentRunLifecycle } = await import('../useAgentRunLifecycle')
const { useConversationPersistence } = await import('../useConversationPersistence')
const { createComposerDraft } = await import('../chatMessageFactory')
const { GuidanceQueue } = await import('../../features/chat/components/GuidanceQueue')
const settings = await loadModelSettings()
if (!settings?.models[0]) throw new Error('Missing real Core fixture model')
const model = settings.models[0]
const uiPreferences = await loadUiPreferences()
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
  const { requestAssistantResponse, waitForRunSettlement } = useAgentRunLifecycle({
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
    requestAssistantResponse,
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
    waitForRunSettlement
  })
  const conversation = conversations.find((item) => item.id === activeConversationId)
  const queue = drafts[activeConversationId ?? 'new']?.queuedMessages ?? []
  return (
    <main
      data-testid="workspace"
      data-conversation-id={activeConversationId ?? ''}
      style={{ width: 760, margin: '100px auto' }}
    >
      <button
        type="button"
        onClick={() =>
          void submission.submitMessage('初始请求', {
            modelId: model.id,
            permissionMode: 'default',
            projectId: null,
            attachments: [],
            skills: []
          })
        }
      >
        开始初始回复
      </button>
      <button
        type="button"
        disabled={!activeConversationId}
        onClick={() => {
          if (!activeConversationId) return
          mutateDraft(activeConversationId, (draft) => ({
            ...draft,
            queuedMessages: [
              ...draft.queuedMessages,
              ...[1, 2, 3].map((index) => ({
                id: crypto.randomUUID(),
                clientMessageId: crypto.randomUUID(),
                content: `排队消息${index}`,
                modelId: model.id,
                permissionMode: 'default' as const,
                projectId: null,
                attachments: [],
                skills: [],
                status: 'pending' as const,
                createdAt: Date.now() + index
              }))
            ]
          }))
        }}
      >
        加入三条队列
      </button>
      <GuidanceQueue
        guideEnabled={false}
        messages={queue}
        queueAutoSendEnabled={submission.queueAutoSendConversationIds.has(
          activeConversationId ?? ''
        )}
        onToggleQueueAutoSend={() => {
          if (activeConversationId) submission.toggleQueueAutoSend(activeConversationId)
        }}
        onDelete={() => undefined}
        onEdit={() => undefined}
        onGuide={() => undefined}
        onMove={() => undefined}
      />
      <output data-testid="queue-count">{queue.length}</output>
      <output data-testid="auto-send">
        {String(submission.queueAutoSendConversationIds.has(activeConversationId ?? ''))}
      </output>
      <pre data-testid="messages">{JSON.stringify(conversation?.messages ?? [])}</pre>
      <pre data-testid="notices">{JSON.stringify(notices)}</pre>
    </main>
  )
}
createRoot(document.getElementById('root')!).render(
  <FrontendConfigProvider>
    <Workspace />
  </FrontendConfigProvider>
)
