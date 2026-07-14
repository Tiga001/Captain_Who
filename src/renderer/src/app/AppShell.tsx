import { useCallback, useEffect, useMemo, useRef, useState, type SetStateAction } from 'react'
import type { AppWindowState } from '@mycopilot/host-api'
import type {
  AgentContextWindowSnapshot,
  AgentEvent,
  AgentInputAttachment,
  AgentProposedAction
} from '@mycopilot/protocol'
import { ResizeHandle } from '../components/layout/ResizeHandle'
import { LeftSidebar } from '../components/sidebar/LeftSidebar'
import { RightSidebar } from '../components/sidebar/RightSidebar'
import { useToast } from '../components/toast/ToastContext'
import { useModelSettings } from '../config/ModelSettingsProvider'
import { useProjectSettings } from '../config/ProjectSettingsProvider'
import { useFrontendConfig } from '../config/FrontendConfigProvider'
import { featureFlags } from '../config/featureFlags'
import { hostClient } from '../host/hostClient'
import { ChatConversationPage } from '../features/chat/ChatConversationPage'
import { NewConversationPage } from '../features/chat/NewConversationPage'
import type { SettingsPageId } from '../features/settings/SettingsPage'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatPermissionMode,
  ChatMessage,
  ChatMessageUiState,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import {
  approveAgentAction,
  cancelAgentAction,
  cancelAgentRun,
  getContextWindowSnapshot,
  listPendingAgentActions,
  onAgentEvent,
  rejectAgentAction,
  startConversationTurn
} from '../features/agent/agentClient'
import { resolveChatPermissions } from '../features/chat/chatPermissions'
import { createAttachmentSummary } from '../features/chat/chatAttachments'
import {
  defaultUiPreferences,
  deleteChatMessages,
  forkConversation,
  loadComposerDrafts,
  loadConversations,
  loadInputAttachments,
  loadUiPreferences,
  saveChatMessageState,
  saveComposerDraft,
  saveConversationMeta,
  saveUiPreferences,
  upsertChatMessages
} from '../features/storage/storageClient'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { NEW_CONVERSATION_DRAFT_ID, THINKING_PLACEHOLDER } from './appConstants'
import type { ActiveRunBinding } from './appTypes'
import { getAgentActionApprovalStatus, getAgentActionId } from './agentActionUtils'
import {
  applyAgentActionDecisionToChatMessage,
  applyAgentActionExecutionToChatMessage,
  applyAgentEventToChatMessage,
  ensureAgentRun,
  settleAgentRunToolActivities,
  shouldTouchConversationForAgentEvent
} from './agentEventReducer'
import {
  createAssistantMessage,
  createComposerDraft,
  createConversationTitle,
  createId,
  createUserMessage,
  mergeConversationMessageFromBackend
} from './chatMessageFactory'
import { useShellLayout } from './useShellLayout'
import { AppShellSettingsView } from './AppShellSettingsView'
import {
  DEFAULT_AGENT_MAX_TOKENS,
  getAppShellPanelStyle,
  getPermissionModeAvailability,
  MainPanelToolbar,
  MaximizedSidebarControls,
  STREAM_DELTA_FLUSH_MS,
  STREAM_DELTA_MAX_BUFFER_CHARS,
  SUPPORTS_NATIVE_FONT_SMOOTHING
} from './AppShellSupport'
import type { PendingMessageDelta, PendingMessageSave } from './AppShellSupport'

type PendingMessageUpsert = {
  messages: ChatMessage[]
  positionOffset: number
}

function buildMessageContentWithAttachments(content: string, attachments: AgentInputAttachment[]) {
  const trimmedContent = content.trim()
  const attachmentSummary = createAttachmentSummary(attachments)
  return [trimmedContent, attachmentSummary].filter(Boolean).join('\n\n')
}

function getEditableLastTurn(conversation: ChatConversation) {
  const messages = conversation.messages
  if (messages.length < 2) return null

  const userIndex = messages.length - 2
  const assistantIndex = messages.length - 1
  const userMessage = messages[userIndex]
  const assistantMessage = messages[assistantIndex]
  if (userMessage?.role !== 'user' || assistantMessage?.role !== 'assistant') return null
  if (assistantMessage.status !== 'sent') return null

  const assistantRunStatus = assistantMessage.agentRun?.status
  const assistantSettled =
    !assistantRunStatus ||
    assistantRunStatus === 'completed' ||
    assistantRunStatus === 'failed' ||
    assistantRunStatus === 'cancelled' ||
    assistantRunStatus === 'idle'
  if (!assistantSettled) return null

  return {
    assistantIndex,
    assistantMessage,
    userIndex,
    userMessage
  }
}

function getActiveRunModelId(conversation: ChatConversation | null) {
  if (!conversation?.modelId) return null

  const latestAssistantMessage = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'assistant')
  if (!latestAssistantMessage) return null

  const runStatus = latestAssistantMessage.agentRun?.status
  const runIsActive =
    latestAssistantMessage.status === 'pending' ||
    runStatus === 'starting' ||
    runStatus === 'running' ||
    runStatus === 'waiting_for_approval'

  return runIsActive ? conversation.modelId : null
}

function getContextWindowSnapshotKey(scopeId: string, model: string) {
  return JSON.stringify([scopeId, model])
}

const DEFAULT_APP_WINDOW_STATE: AppWindowState = {
  isFullScreen: false,
  isMaximized: false
}

export function AppShell() {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const { enabledModels, models } = useModelSettings()
  const { projects, deleteProject, renameProject, showProjectInFolder, togglePinProject } =
    useProjectSettings()
  const {
    leftOpen,
    leftWidth,
    resizeSide,
    rightMaximized,
    rightOpen,
    rightWidth,
    shellRef,
    toggleLeftSidebar,
    toggleRightSidebar,
    toggleRightSidebarMaximized
  } = useShellLayout()
  const [view, setView] = useState<'workspace' | 'settings'>('workspace')
  const [appWindowState, setAppWindowState] = useState<AppWindowState>(DEFAULT_APP_WINDOW_STATE)
  const [settingsInitialPage, setSettingsInitialPage] = useState<SettingsPageId>('general')
  const [uiPreferences, setUiPreferences] = useState<UiPreferencesSnapshot>(() =>
    defaultUiPreferences()
  )
  const [conversations, setConversations] = useState<ChatConversation[]>([])
  const conversationsRef = useRef<ChatConversation[]>([])
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null)
  const [activeConversationInitialScrollTop, setActiveConversationInitialScrollTop] = useState<
    number | null
  >(null)
  const [conversationScrollToBottomSignal, setConversationScrollToBottomSignal] = useState(0)
  const [scrollTargetMessageId, setScrollTargetMessageId] = useState<string | null>(null)
  const activeConversationIdRef = useRef<string | null>(null)
  const conversationScrollPositionsRef = useRef<Map<string, number>>(new Map())
  const activeRunBindingsRef = useRef<Map<string, ActiveRunBinding>>(new Map())
  const bufferedAgentEventsRef = useRef<Map<string, AgentEvent[]>>(new Map())
  const pendingMessageDeltasRef = useRef<Map<string, PendingMessageDelta>>(new Map())
  const conversationSaveQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingConversationSavesRef = useRef<Map<string, ChatConversation>>(new Map())
  const messageUpsertQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingMessageUpsertsRef = useRef<Map<string, PendingMessageUpsert[]>>(new Map())
  const messageSaveQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingMessageSavesRef = useRef<Map<string, PendingMessageSave>>(new Map())
  const pendingActionsHydratedRef = useRef(false)
  const cancelledPendingMessageIdsRef = useRef<Set<string>>(new Set())
  const cancelledRunIdsRef = useRef<Set<string>>(new Set())
  const editSubmissionSeqRef = useRef(0)
  const contextWindowRequestSeqRef = useRef(0)
  const contextWindowEventSeqRef = useRef<Map<string, number>>(new Map())
  const [contextWindowSnapshots, setContextWindowSnapshots] = useState<
    Record<string, AgentContextWindowSnapshot>
  >({})
  const [drafts, setDrafts] = useState<Record<string, ChatComposerDraft>>({
    [NEW_CONVERSATION_DRAFT_ID]: createComposerDraft()
  })
  const activeConversation = useMemo(
    () => conversations.find((conversation) => conversation.id === activeConversationId) ?? null,
    [activeConversationId, conversations]
  )
  const activeDraftId = activeConversation?.id ?? NEW_CONVERSATION_DRAFT_ID
  const activeDraft =
    drafts[activeDraftId] ??
    createComposerDraft({ projectId: activeConversation?.projectId ?? null })
  const activeDraftSelectedModel = useMemo(
    () =>
      enabledModels.find((model) => model.id === activeDraft.modelId) ?? enabledModels[0] ?? null,
    [activeDraft.modelId, enabledModels]
  )
  const activeRunModelId = useMemo(
    () => getActiveRunModelId(activeConversation),
    [activeConversation]
  )
  // The composer selects the next run. Capacity reporting stays pinned to the immutable model of
  // the current run until that run reaches a terminal state.
  const contextWindowModel = useMemo(
    () =>
      activeRunModelId
        ? (models.find((model) => model.id === activeRunModelId) ?? null)
        : activeDraftSelectedModel,
    [activeDraftSelectedModel, activeRunModelId, models]
  )
  const activeContextWindowKey = activeConversation?.id ?? NEW_CONVERSATION_DRAFT_ID
  const contextWindowModelProviderPath = contextWindowModel
    ? (contextWindowModel.providerPath ?? contextWindowModel.id)
    : null
  const activeContextWindowSnapshotKey = contextWindowModelProviderPath
    ? getContextWindowSnapshotKey(activeContextWindowKey, contextWindowModelProviderPath)
    : null
  const contextWindowIndicatorEnabled =
    featureFlags.contextWindowIndicator && uiPreferences.showContextWindowUsage
  const activeContextWindowSnapshot =
    contextWindowIndicatorEnabled && activeContextWindowSnapshotKey
      ? contextWindowSnapshots[activeContextWindowSnapshotKey]
      : undefined
  const permissionModeAvailability = getPermissionModeAvailability(uiPreferences)
  const hasUnreadConversations = conversations.some(
    (conversation) => !conversation.archivedAt && Boolean(conversation.unreadAt)
  )
  const rightSidebarTerminalProjectId = activeConversation
    ? activeConversation.projectId
    : activeDraft.projectId
  const rightSidebarTerminalProject = useMemo(() => {
    if (!rightSidebarTerminalProjectId) return null

    return projects.find((project) => project.id === rightSidebarTerminalProjectId) ?? null
  }, [rightSidebarTerminalProjectId, projects])
  const rightSidebarTerminalPath = rightSidebarTerminalProject?.path?.trim() || undefined

  // Agent tool events arrive faster than React state commits. Keep the ref and state in one
  // update path so an older render snapshot cannot overwrite newer tool-call results.
  const setConversationsWithRef = useCallback((value: SetStateAction<ChatConversation[]>) => {
    const nextConversations =
      typeof value === 'function'
        ? (value as (currentConversations: ChatConversation[]) => ChatConversation[])(
            conversationsRef.current
          )
        : value

    conversationsRef.current = nextConversations
    setConversations(nextConversations)
  }, [])

  useEffect(() => {
    activeConversationIdRef.current = activeConversationId
  }, [activeConversationId])

  useEffect(() => {
    if (!contextWindowIndicatorEnabled || !contextWindowModel || !contextWindowModelProviderPath) {
      return undefined
    }

    const requestSequence = contextWindowRequestSeqRef.current + 1
    contextWindowRequestSeqRef.current = requestSequence
    const scopeKey = activeConversation?.id ?? NEW_CONVERSATION_DRAFT_ID
    const snapshotKey = getContextWindowSnapshotKey(scopeKey, contextWindowModelProviderPath)
    const eventSequenceAtRequest = contextWindowEventSeqRef.current.get(snapshotKey) ?? 0
    let cancelled = false

    void getContextWindowSnapshot({
      conversationId: activeConversation?.id,
      projectId: activeConversation?.projectId ?? activeDraft.projectId,
      modelId: contextWindowModel.id,
      maxTokens: DEFAULT_AGENT_MAX_TOKENS,
      permissions: resolveChatPermissions(
        activeDraft.permissionMode,
        uiPreferences.customPermissions
      )
    })
      .then(({ snapshot }) => {
        if (cancelled || contextWindowRequestSeqRef.current !== requestSequence) return
        if ((contextWindowEventSeqRef.current.get(snapshotKey) ?? 0) !== eventSequenceAtRequest) {
          return
        }
        setContextWindowSnapshots((current) => {
          if (snapshot?.model === contextWindowModelProviderPath) {
            return { ...current, [snapshotKey]: snapshot }
          }
          if (!(snapshotKey in current)) return current
          const next = { ...current }
          delete next[snapshotKey]
          return next
        })
      })
      .catch((error) => {
        if (!cancelled) console.error('Failed to inspect context window', error)
      })

    return () => {
      cancelled = true
    }
  }, [
    activeConversation?.id,
    activeConversation?.projectId,
    activeDraft.permissionMode,
    activeDraft.projectId,
    contextWindowModel,
    contextWindowModelProviderPath,
    contextWindowIndicatorEnabled,
    uiPreferences.customPermissions
  ])

  useEffect(() => {
    let cancelled = false
    const unsubscribe = hostClient.app.onWindowStateChange(setAppWindowState)

    void hostClient.app
      .getWindowState()
      .then((state) => {
        if (!cancelled) {
          setAppWindowState(state)
        }
      })
      .catch((error) => {
        console.error('Failed to load app window state', error)
      })

    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    void Promise.all([loadUiPreferences(), loadConversations(), loadComposerDrafts()]).then(
      ([preferences, storedConversations, storedDrafts]) => {
        if (cancelled) return
        setUiPreferences(preferences)
        setConversationsWithRef(storedConversations)
        setDrafts({
          [NEW_CONVERSATION_DRAFT_ID]: createComposerDraft(),
          ...storedDrafts
        })
        setActiveConversationId(
          storedConversations.find((conversation) => !conversation.archivedAt)?.id ?? null
        )
      }
    )
    return () => {
      cancelled = true
    }
  }, [setConversationsWithRef])

  const updateUiPreferences = useCallback((patch: Partial<UiPreferencesSnapshot>) => {
    setUiPreferences((currentPreferences) => {
      const nextPreferences = {
        ...currentPreferences,
        ...patch,
        updatedAt: Date.now()
      }
      void saveUiPreferences(nextPreferences)
      return nextPreferences
    })
  }, [])

  const updateDraft = useCallback((scopeId: string, draft: ChatComposerDraft) => {
    setDrafts((currentDrafts) => ({
      ...currentDrafts,
      [scopeId]: draft
    }))
    void saveComposerDraft(scopeId, draft)
  }, [])

  const clearPendingMessageDelta = useCallback((runId: string) => {
    const pendingDelta = pendingMessageDeltasRef.current.get(runId)
    if (!pendingDelta) return

    window.clearTimeout(pendingDelta.timerId)
    pendingMessageDeltasRef.current.delete(runId)
  }, [])

  const cleanupRunBinding = useCallback(
    (runId: string) => {
      activeRunBindingsRef.current.delete(runId)
      bufferedAgentEventsRef.current.delete(runId)
      clearPendingMessageDelta(runId)
    },
    [clearPendingMessageDelta]
  )

  const cancelBackendAgentRun = useCallback((runId: string) => {
    void cancelAgentRun(runId).catch((error) => {
      console.error('Failed to cancel agent run', error)
    })
  }, [])

  const waitForConversationSaves = useCallback(async (conversationId: string) => {
    while (true) {
      const pendingSave = conversationSaveQueuesRef.current.get(conversationId)
      if (!pendingSave) return
      await pendingSave
    }
  }, [])

  const waitForMessageUpserts = useCallback(async (conversationId: string) => {
    while (true) {
      const pendingUpsert = messageUpsertQueuesRef.current.get(conversationId)
      if (!pendingUpsert) return
      await pendingUpsert
    }
  }, [])

  const waitForMessageStateSaves = useCallback(async (conversationId: string) => {
    while (true) {
      const pendingSaves = [...messageSaveQueuesRef.current.entries()]
        .filter(([key]) => key.startsWith(`${conversationId}:`))
        .map(([, pendingSave]) => pendingSave)
      if (pendingSaves.length === 0) return
      await Promise.allSettled(pendingSaves)
    }
  }, [])

  // Sending a message only needs conversation metadata here; full conversation saves delete and
  // reinsert all messages, which can overwrite concurrent agent/tool message-state updates.
  const enqueueConversationMetaSave = useCallback((conversation: ChatConversation) => {
    const conversationId = conversation.id
    pendingConversationSavesRef.current.set(conversationId, conversation)

    if (conversationSaveQueuesRef.current.has(conversationId)) {
      return
    }

    const drainSaves = async () => {
      while (true) {
        const payload = pendingConversationSavesRef.current.get(conversationId)
        if (!payload) return

        pendingConversationSavesRef.current.delete(conversationId)
        try {
          await saveConversationMeta(payload)
        } catch (error) {
          console.error('Failed to save conversation metadata to SQLite', error)
        }
      }
    }

    const nextSave = drainSaves().finally(() => {
      conversationSaveQueuesRef.current.delete(conversationId)
    })
    conversationSaveQueuesRef.current.set(conversationId, nextSave)
  }, [])

  const enqueueChatMessagesUpsert = useCallback(
    (conversationId: string, messages: ChatMessage[], positionOffset: number) => {
      const pendingUpserts = pendingMessageUpsertsRef.current.get(conversationId) ?? []
      pendingMessageUpsertsRef.current.set(conversationId, [
        ...pendingUpserts,
        {
          messages,
          positionOffset
        }
      ])

      if (messageUpsertQueuesRef.current.has(conversationId)) {
        return
      }

      const drainUpserts = async () => {
        while (true) {
          const pendingUpserts = pendingMessageUpsertsRef.current.get(conversationId) ?? []
          const payload = pendingUpserts[0]
          if (!payload) return

          const remainingUpserts = pendingUpserts.slice(1)
          if (remainingUpserts.length > 0) {
            pendingMessageUpsertsRef.current.set(conversationId, remainingUpserts)
          } else {
            pendingMessageUpsertsRef.current.delete(conversationId)
          }

          try {
            await waitForConversationSaves(conversationId)
            await upsertChatMessages(conversationId, payload.messages, payload.positionOffset)
          } catch (error) {
            console.error('Failed to upsert chat messages to SQLite', error)
          }
        }
      }

      const nextUpsert = drainUpserts().finally(() => {
        messageUpsertQueuesRef.current.delete(conversationId)
      })
      messageUpsertQueuesRef.current.set(conversationId, nextUpsert)
    },
    [waitForConversationSaves]
  )

  const enqueueChatMessageStateSave = useCallback(
    (conversationId: string, message: ChatMessage) => {
      const key = `${conversationId}:${message.id}`
      pendingMessageSavesRef.current.set(key, { conversationId, message })

      if (messageSaveQueuesRef.current.has(key)) {
        return
      }

      const drainSaves = async () => {
        while (true) {
          const payload = pendingMessageSavesRef.current.get(key)
          if (!payload) return

          pendingMessageSavesRef.current.delete(key)
          try {
            await waitForConversationSaves(payload.conversationId)
            await waitForMessageUpserts(payload.conversationId)
            await saveChatMessageState(payload.conversationId, payload.message)
          } catch (error) {
            console.error('Failed to save chat message state to SQLite', error)
          }
        }
      }

      const nextSave = drainSaves().finally(() => {
        messageSaveQueuesRef.current.delete(key)
      })
      messageSaveQueuesRef.current.set(key, nextSave)
    },
    [waitForConversationSaves, waitForMessageUpserts]
  )

  const updateAssistantMessage = useCallback(
    (
      conversationId: string,
      messageId: string,
      updater: (message: ChatMessage) => ChatMessage,
      options: { persist?: boolean; touchConversation?: boolean } = {}
    ) => {
      let messageToSave: ChatMessage | null = null
      let conversationMetaToSave: ChatConversation | null = null
      const timestamp = Date.now()

      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation

        const nextConversation = {
          ...conversation,
          messages: conversation.messages.map((message) => {
            if (message.id !== messageId) return message
            messageToSave = updater(message)
            return messageToSave
          }),
          updatedAt: options.touchConversation ? timestamp : conversation.updatedAt
        }

        conversationMetaToSave = nextConversation
        return nextConversation
      })

      setConversationsWithRef(nextConversations)

      if (messageToSave && options.persist !== false) {
        enqueueChatMessageStateSave(conversationId, messageToSave)
      }
      if (options.touchConversation && conversationMetaToSave) {
        void saveConversationMeta(conversationMetaToSave)
      }
    },
    [enqueueChatMessageStateSave, setConversationsWithRef]
  )

  const flushPendingMessageDelta = useCallback(
    (runId: string) => {
      const pendingDelta = pendingMessageDeltasRef.current.get(runId)
      if (!pendingDelta) return

      window.clearTimeout(pendingDelta.timerId)
      pendingMessageDeltasRef.current.delete(runId)
      updateAssistantMessage(
        pendingDelta.conversationId,
        pendingDelta.messageId,
        (message) =>
          applyAgentEventToChatMessage(message, {
            type: 'message_delta',
            runId,
            streamId: pendingDelta.streamId,
            delta: pendingDelta.delta
          }),
        { touchConversation: false }
      )
    },
    [updateAssistantMessage]
  )

  const bufferMessageDelta = useCallback(
    (
      conversationId: string,
      messageId: string,
      agentEvent: AgentEvent & { type: 'message_delta' }
    ) => {
      const pendingDelta = pendingMessageDeltasRef.current.get(agentEvent.runId)

      if (pendingDelta) {
        pendingDelta.conversationId = conversationId
        pendingDelta.messageId = messageId
        pendingDelta.delta += agentEvent.delta

        if (
          pendingDelta.delta.includes('\n') ||
          pendingDelta.delta.length >= STREAM_DELTA_MAX_BUFFER_CHARS
        ) {
          flushPendingMessageDelta(agentEvent.runId)
        }
        return
      }

      const timerId = window.setTimeout(() => {
        flushPendingMessageDelta(agentEvent.runId)
      }, STREAM_DELTA_FLUSH_MS)
      pendingMessageDeltasRef.current.set(agentEvent.runId, {
        conversationId,
        delta: agentEvent.delta,
        messageId,
        streamId: agentEvent.streamId,
        timerId
      })
    },
    [flushPendingMessageDelta]
  )

  useEffect(() => {
    if (pendingActionsHydratedRef.current || conversations.length === 0) return
    pendingActionsHydratedRef.current = true

    void listPendingAgentActions()
      .then((pendingActions) => {
        for (const pendingAction of pendingActions) {
          if (!pendingAction.conversationId || !pendingAction.assistantMessageId) continue
          const conversationExists = conversationsRef.current.some(
            (conversation) => conversation.id === pendingAction.conversationId
          )
          if (!conversationExists) continue

          activeRunBindingsRef.current.set(pendingAction.runId, {
            conversationId: pendingAction.conversationId,
            pendingMessageId: pendingAction.assistantMessageId
          })
          updateAssistantMessage(
            pendingAction.conversationId,
            pendingAction.assistantMessageId,
            (message) =>
              applyAgentEventToChatMessage(message, {
                type: 'approval_required',
                runId: pendingAction.runId,
                action: pendingAction.action
              }),
            { touchConversation: false }
          )
        }
      })
      .catch((error) => {
        console.error('Failed to hydrate pending agent actions', error)
      })
  }, [conversations.length, updateAssistantMessage])

  const handleBoundAgentEvent = useCallback(
    (conversationId: string, assistantMessageId: string, agentEvent: AgentEvent) => {
      if (agentEvent.runId && cancelledRunIdsRef.current.has(agentEvent.runId)) return
      if (cancelledPendingMessageIdsRef.current.has(assistantMessageId)) return

      if (agentEvent.type === 'message_delta') {
        bufferMessageDelta(conversationId, assistantMessageId, agentEvent)
        return
      }

      if (agentEvent.type === 'tool_input_progress') return

      if (
        agentEvent.type === 'file_write_preview_updated' ||
        agentEvent.type === 'file_write_preview_cleared'
      ) {
        updateAssistantMessage(
          conversationId,
          assistantMessageId,
          (message) => applyAgentEventToChatMessage(message, agentEvent),
          { persist: false, touchConversation: false }
        )
        return
      }

      if (agentEvent.runId) {
        flushPendingMessageDelta(agentEvent.runId)
      }

      updateAssistantMessage(
        conversationId,
        assistantMessageId,
        (message) => applyAgentEventToChatMessage(message, agentEvent),
        { touchConversation: shouldTouchConversationForAgentEvent(agentEvent) }
      )

      if (agentEvent.type === 'done') {
        if (agentEvent.success && agentEvent.status !== 'waiting_for_approval') {
          const completedAt = Date.now()
          let conversationToSave: ChatConversation | null = null
          const nextConversations = conversationsRef.current.map((conversation) => {
            if (
              conversation.id !== conversationId ||
              activeConversationIdRef.current === conversationId
            ) {
              return conversation
            }

            conversationToSave = {
              ...conversation,
              unreadAt: completedAt
            }
            return conversationToSave
          })

          if (conversationToSave) {
            setConversationsWithRef(nextConversations)
            void saveConversationMeta(conversationToSave)
          }
        }

        if (agentEvent.status !== 'waiting_for_approval') {
          cleanupRunBinding(agentEvent.runId)
        }
      }

      if (agentEvent.type === 'error' && !agentEvent.recoverable && agentEvent.runId) {
        cleanupRunBinding(agentEvent.runId)
      }
    },
    [
      bufferMessageDelta,
      cleanupRunBinding,
      flushPendingMessageDelta,
      setConversationsWithRef,
      updateAssistantMessage
    ]
  )

  useEffect(() => {
    return onAgentEvent((agentEvent) => {
      if (agentEvent.type === 'context_window_updated') {
        const conversationId = agentEvent.conversationId
        if (contextWindowIndicatorEnabled && conversationId) {
          const snapshotKey = getContextWindowSnapshotKey(conversationId, agentEvent.snapshot.model)
          contextWindowEventSeqRef.current.set(
            snapshotKey,
            (contextWindowEventSeqRef.current.get(snapshotKey) ?? 0) + 1
          )
          setContextWindowSnapshots((current) => ({
            ...current,
            [snapshotKey]: agentEvent.snapshot
          }))
        }
        return
      }

      const runId = agentEvent.runId
      if (!runId) return
      if (cancelledRunIdsRef.current.has(runId)) return

      const binding = activeRunBindingsRef.current.get(runId)
      if (!binding) {
        const bufferedEvents = bufferedAgentEventsRef.current.get(runId) ?? []
        bufferedAgentEventsRef.current.set(runId, [...bufferedEvents, agentEvent])
        return
      }

      handleBoundAgentEvent(binding.conversationId, binding.pendingMessageId, agentEvent)
    })
  }, [contextWindowIndicatorEnabled, handleBoundAgentEvent])

  useEffect(() => {
    const pendingMessageDeltas = pendingMessageDeltasRef.current
    return () => {
      for (const pendingDelta of pendingMessageDeltas.values()) {
        window.clearTimeout(pendingDelta.timerId)
      }
      pendingMessageDeltas.clear()
    }
  }, [])

  const requestAssistantResponse = useCallback(
    async (
      conversationId: string,
      userMessageId: string,
      assistantMessageId: string,
      content: string,
      modelId: string,
      projectId: string | null,
      permissionMode: ChatPermissionMode,
      attachments: ChatSubmitOptions['attachments'],
      title?: string
    ) => {
      updateAssistantMessage(conversationId, assistantMessageId, (message) => ({
        ...message,
        agentRun: ensureAgentRun(message.agentRun, null, 'starting')
      }))

      try {
        const startOutput = await startConversationTurn({
          assistantMessageId,
          attachments,
          content,
          contextWindowIndicatorEnabled,
          conversationId,
          maxTokens: DEFAULT_AGENT_MAX_TOKENS,
          modelId,
          permissions: resolveChatPermissions(permissionMode, uiPreferences.customPermissions),
          projectId,
          title,
          userMessageId
        })

        if (cancelledPendingMessageIdsRef.current.has(assistantMessageId)) {
          cancelledPendingMessageIdsRef.current.delete(assistantMessageId)
          cancelledRunIdsRef.current.add(startOutput.runId)
          cancelBackendAgentRun(startOutput.runId)
          bufferedAgentEventsRef.current.delete(startOutput.runId)
          return
        }

        const resolvedConversationId = startOutput.conversationId
        const resolvedAssistantMessageId = startOutput.assistantMessageId

        const nextConversations = conversationsRef.current.map((conversation) =>
          conversation.id === conversationId
            ? {
                ...conversation,
                id: resolvedConversationId,
                messages: conversation.messages.map((message) => {
                  if (message.id === userMessageId) {
                    return mergeConversationMessageFromBackend(message, startOutput.userMessage)
                  }

                  if (message.id === assistantMessageId) {
                    const mergedMessage = mergeConversationMessageFromBackend(
                      message,
                      startOutput.assistantMessage
                    )
                    return {
                      ...mergedMessage,
                      content: mergedMessage.content || message.content || THINKING_PLACEHOLDER,
                      status: 'pending' as const,
                      agentRun: ensureAgentRun(mergedMessage.agentRun, startOutput.runId, 'running')
                    }
                  }

                  return message
                })
              }
            : conversation
        )
        setConversationsWithRef(nextConversations)

        if (resolvedConversationId !== conversationId) {
          setActiveConversationId((currentActiveConversationId) =>
            currentActiveConversationId === conversationId
              ? resolvedConversationId
              : currentActiveConversationId
          )
          if (activeConversationIdRef.current === conversationId) {
            activeConversationIdRef.current = resolvedConversationId
          }
        }

        activeRunBindingsRef.current.set(startOutput.runId, {
          conversationId: resolvedConversationId,
          pendingMessageId: resolvedAssistantMessageId
        })

        const bufferedEvents = bufferedAgentEventsRef.current.get(startOutput.runId) ?? []
        bufferedAgentEventsRef.current.delete(startOutput.runId)
        bufferedEvents.forEach((agentEvent) => {
          handleBoundAgentEvent(resolvedConversationId, resolvedAssistantMessageId, agentEvent)
        })
      } catch (error) {
        if (cancelledPendingMessageIdsRef.current.has(assistantMessageId)) {
          cancelledPendingMessageIdsRef.current.delete(assistantMessageId)
          return
        }

        const message = error instanceof Error ? error.message : String(error)
        updateAssistantMessage(
          conversationId,
          assistantMessageId,
          (currentMessage) => ({
            ...currentMessage,
            content: message,
            status: 'error',
            agentRun: {
              ...ensureAgentRun(currentMessage.agentRun, null, 'failed'),
              error: message
            }
          }),
          { touchConversation: true }
        )
      }
    },
    [
      cancelBackendAgentRun,
      contextWindowIndicatorEnabled,
      handleBoundAgentEvent,
      setConversationsWithRef,
      uiPreferences.customPermissions,
      updateAssistantMessage
    ]
  )

  const submitMessage = useCallback(
    (message: string, options: ChatSubmitOptions) => {
      const now = Date.now()
      const conversationId = activeConversation?.id ?? createId('conversation')
      const userMessage = createUserMessage(message, options.attachments ?? [])
      const assistantMessage = createAssistantMessage(THINKING_PLACEHOLDER, 'pending')
      const title = createConversationTitle(message, t('chat.newConversation'))
      const conversationToSave: ChatConversation = activeConversation
        ? {
            ...activeConversation,
            modelId: options.modelId,
            messages: [...activeConversation.messages, userMessage, assistantMessage],
            updatedAt: now
          }
        : {
            id: conversationId,
            projectId: options.projectId,
            modelId: options.modelId,
            title,
            messages: [userMessage, assistantMessage],
            createdAt: now,
            updatedAt: now,
            pinnedAt: null,
            archivedAt: null,
            unreadAt: null
          }

      const existingConversation = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      const nextConversations = existingConversation
        ? conversationsRef.current.map((conversation) =>
            conversation.id === conversationId ? conversationToSave : conversation
          )
        : [conversationToSave, ...conversationsRef.current]

      setConversationsWithRef(nextConversations)
      enqueueConversationMetaSave(conversationToSave)
      enqueueChatMessagesUpsert(
        conversationId,
        [userMessage, assistantMessage],
        activeConversation?.messages.length ?? 0
      )
      activeConversationIdRef.current = conversationId
      setScrollTargetMessageId(null)
      setActiveConversationInitialScrollTop(null)
      setConversationScrollToBottomSignal((signal) => signal + 1)
      setActiveConversationId(conversationId)
      updateDraft(
        conversationId,
        createComposerDraft({
          modelId: options.modelId,
          permissionMode: options.permissionMode,
          projectId: options.projectId
        })
      )
      void requestAssistantResponse(
        conversationId,
        userMessage.id,
        assistantMessage.id,
        message,
        options.modelId,
        activeConversation?.projectId ?? options.projectId,
        options.permissionMode,
        options.attachments,
        activeConversation ? undefined : title
      )
    },
    [
      activeConversation,
      enqueueChatMessagesUpsert,
      enqueueConversationMetaSave,
      requestAssistantResponse,
      setConversationsWithRef,
      t,
      updateDraft
    ]
  )

  const submitEditedLastUserMessage = useCallback(
    async (messageId: string, content: string) => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) {
        throw new Error(t('chat.editNoConversation'))
      }

      const conversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      if (!conversation) {
        throw new Error(t('chat.editConversationMissing'))
      }

      const editableTurn = getEditableLastTurn(conversation)
      if (!editableTurn || editableTurn.userMessage.id !== messageId) {
        throw new Error(t('chat.editMessageUnavailable'))
      }

      const attachmentIds =
        editableTurn.userMessage.attachments?.map((attachment) => attachment.id) ?? []
      const attachments = await loadInputAttachments(attachmentIds)

      await waitForConversationSaves(conversationId)
      await waitForMessageUpserts(conversationId)

      const latestConversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      if (!latestConversation) {
        throw new Error(t('chat.editConversationMissing'))
      }

      const latestEditableTurn = getEditableLastTurn(latestConversation)
      if (!latestEditableTurn || latestEditableTurn.userMessage.id !== messageId) {
        throw new Error(t('chat.editConversationChanged'))
      }

      const messageContent = buildMessageContentWithAttachments(content, attachments)
      if (!messageContent.trim()) {
        throw new Error(t('chat.emptyMessage'))
      }
      if (!activeDraftSelectedModel) {
        throw new Error(t('chat.noEnabledModels'))
      }
      if (
        attachments.some((attachment) => attachment.kind === 'image') &&
        !activeDraftSelectedModel.supportsImage
      ) {
        throw new Error(t('chat.unsupportedImageWarning'))
      }

      const now = Date.now()
      const modelId = activeDraftSelectedModel.id
      const permissionMode = activeDraft.permissionMode
      const userMessage = createUserMessage(messageContent, attachments)
      const assistantMessage = createAssistantMessage(THINKING_PLACEHOLDER, 'pending')
      const messagesBeforeEditedTurn = latestConversation.messages.slice(
        0,
        latestEditableTurn.userIndex
      )
      const nextConversation: ChatConversation = {
        ...latestConversation,
        messages: [...messagesBeforeEditedTurn, userMessage, assistantMessage],
        modelId,
        updatedAt: now,
        unreadAt: null
      }

      const oldRunId = latestEditableTurn.assistantMessage.agentRun?.runId
      if (oldRunId) {
        cancelledRunIdsRef.current.add(oldRunId)
        cleanupRunBinding(oldRunId)
      }

      cancelledPendingMessageIdsRef.current.add(latestEditableTurn.assistantMessage.id)
      const submissionSeq = (editSubmissionSeqRef.current += 1)
      setScrollTargetMessageId(null)
      setActiveConversationInitialScrollTop(null)
      setConversationScrollToBottomSignal((signal) => signal + 1)
      setConversationsWithRef((currentConversations) =>
        currentConversations.map((candidate) =>
          candidate.id === conversationId ? nextConversation : candidate
        )
      )
      updateDraft(
        conversationId,
        createComposerDraft({
          modelId,
          permissionMode,
          projectId: latestConversation.projectId
        })
      )

      void (async () => {
        try {
          await saveConversationMeta(nextConversation)
          await deleteChatMessages(conversationId, [
            latestEditableTurn.userMessage.id,
            latestEditableTurn.assistantMessage.id
          ])
          await upsertChatMessages(
            conversationId,
            [userMessage, assistantMessage],
            latestEditableTurn.userIndex
          )

          if (editSubmissionSeqRef.current !== submissionSeq) return

          await requestAssistantResponse(
            conversationId,
            userMessage.id,
            assistantMessage.id,
            content,
            modelId,
            latestConversation.projectId,
            permissionMode,
            attachments,
            undefined
          )
        } catch (error) {
          if (editSubmissionSeqRef.current !== submissionSeq) return
          const errorMessage = error instanceof Error ? error.message : String(error)
          updateAssistantMessage(
            conversationId,
            assistantMessage.id,
            (currentMessage) => ({
              ...currentMessage,
              content: errorMessage,
              status: 'error',
              agentRun: {
                ...ensureAgentRun(currentMessage.agentRun, null, 'failed'),
                error: errorMessage
              }
            }),
            { touchConversation: true }
          )
        }
      })()
    },
    [
      activeDraft.permissionMode,
      activeDraftSelectedModel,
      cleanupRunBinding,
      requestAssistantResponse,
      setConversationsWithRef,
      t,
      updateDraft,
      updateAssistantMessage,
      waitForConversationSaves,
      waitForMessageUpserts
    ]
  )

  const selectConversation = useCallback(
    (conversationId: string, messageId?: string | null) => {
      activeConversationIdRef.current = conversationId
      const selectedConversation = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      const shouldRestoreRememberedPosition = !messageId && !selectedConversation?.unreadAt

      setScrollTargetMessageId(messageId ?? null)
      setActiveConversationInitialScrollTop(
        shouldRestoreRememberedPosition
          ? (conversationScrollPositionsRef.current.get(conversationId) ?? null)
          : null
      )

      let conversationToSave: ChatConversation | null = null
      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId || !conversation.unreadAt) return conversation

        conversationToSave = {
          ...conversation,
          unreadAt: null
        }
        return conversationToSave
      })

      if (conversationToSave) {
        setConversationsWithRef(nextConversations)
        void saveConversationMeta(conversationToSave)
      }

      setActiveConversationId(conversationId)
    },
    [setConversationsWithRef]
  )

  const continueInNewTask = useCallback(
    async (sourceConversationId: string, throughAssistantMessageId: string) => {
      try {
        const newConversation = await forkConversation(
          sourceConversationId,
          throughAssistantMessageId,
          createId('conversation-fork-request')
        )
        const sourceDraft =
          drafts[sourceConversationId] ??
          createComposerDraft({
            modelId: newConversation.modelId ?? undefined,
            projectId: newConversation.projectId
          })
        const newDraft = createComposerDraft({
          modelId: newConversation.modelId ?? sourceDraft.modelId,
          permissionMode: sourceDraft.permissionMode,
          projectId: newConversation.projectId
        })

        setConversationsWithRef((currentConversations) => [
          newConversation,
          ...currentConversations.filter((conversation) => conversation.id !== newConversation.id)
        ])
        setDrafts((currentDrafts) => ({
          ...currentDrafts,
          [newConversation.id]: newDraft
        }))
        void saveComposerDraft(newConversation.id, newDraft)
        conversationScrollPositionsRef.current.delete(newConversation.id)
        activeConversationIdRef.current = newConversation.id
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)
        setConversationScrollToBottomSignal((signal) => signal + 1)
        setActiveConversationId(newConversation.id)
        setView('workspace')
      } catch (error) {
        console.error('Failed to continue conversation in a new task', error)
        showToast(t('chat.continueInNewTaskFailed'))
      }
    },
    [drafts, setConversationsWithRef, showToast, t]
  )

  const rememberConversationScrollPosition = useCallback(
    (conversationId: string, scrollTop: number) => {
      conversationScrollPositionsRef.current.set(conversationId, scrollTop)
    },
    []
  )

  const patchConversation = useCallback(
    (conversationId: string, patch: Partial<ChatConversation>) => {
      let nextConversation: ChatConversation | null = null
      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation

        nextConversation = { ...conversation, ...patch }
        return nextConversation
      })

      if (nextConversation) {
        setConversationsWithRef(nextConversations)
        void saveConversationMeta(nextConversation)
      }
    },
    [setConversationsWithRef]
  )

  const archiveConversations = useCallback(
    (predicate: (conversation: ChatConversation) => boolean) => {
      const archivedAt = Date.now()
      setConversationsWithRef((currentConversations) =>
        currentConversations.map((conversation) => {
          if (conversation.archivedAt || !predicate(conversation)) return conversation

          const nextConversation = {
            ...conversation,
            archivedAt,
            unreadAt: null
          }
          void saveConversationMeta(nextConversation)
          return nextConversation
        })
      )
    },
    [setConversationsWithRef]
  )

  const removeProject = useCallback(
    async (projectId: string): Promise<boolean> => {
      const projectConversations = conversationsRef.current.filter(
        (conversation) => conversation.projectId === projectId
      )
      const conversationIds = new Set(projectConversations.map((conversation) => conversation.id))
      const runIds = new Set<string>()
      const actionIds = new Set<string>()

      editSubmissionSeqRef.current += 1
      for (const conversation of projectConversations) {
        for (const message of conversation.messages) {
          if (message.role !== 'assistant' || message.status !== 'pending') continue

          cancelledPendingMessageIdsRef.current.add(message.id)
          if (message.agentRun?.runId) runIds.add(message.agentRun.runId)
          for (const action of message.agentRun?.approvals ?? []) {
            if (getAgentActionApprovalStatus(action) === 'required') {
              actionIds.add(getAgentActionId(action))
            }
          }
        }
      }
      for (const [runId, binding] of activeRunBindingsRef.current) {
        if (conversationIds.has(binding.conversationId)) runIds.add(runId)
      }

      runIds.forEach((runId) => {
        cancelledRunIdsRef.current.add(runId)
        cleanupRunBinding(runId)
      })
      await Promise.allSettled([
        ...[...runIds].map((runId) => cancelAgentRun(runId)),
        ...[...actionIds].map((actionId) => cancelAgentAction(actionId)),
        ...[...conversationIds].map(async (conversationId) => {
          await waitForConversationSaves(conversationId)
          await waitForMessageUpserts(conversationId)
          await waitForMessageStateSaves(conversationId)
        })
      ])

      try {
        await deleteProject(projectId)
      } catch (error) {
        console.error('Failed to remove project', error)
        const cancelledAt = Date.now()
        const reconciledConversations = conversationsRef.current.map((conversation) => {
          if (conversation.projectId !== projectId) return conversation

          return {
            ...conversation,
            messages: conversation.messages.map((message) => {
              if (message.role !== 'assistant' || message.status !== 'pending') return message

              const currentRun = ensureAgentRun(
                message.agentRun,
                message.agentRun?.runId ?? null,
                'cancelled'
              )
              const cancelledMessage: ChatMessage = {
                ...message,
                content:
                  message.content && message.content !== THINKING_PLACEHOLDER
                    ? message.content
                    : '',
                status: 'sent',
                agentRun: settleAgentRunToolActivities(
                  {
                    ...currentRun,
                    completedAt: cancelledAt,
                    todo: undefined
                  },
                  'cancelled',
                  cancelledAt
                )
              }
              enqueueChatMessageStateSave(conversation.id, cancelledMessage)
              return cancelledMessage
            })
          }
        })
        setConversationsWithRef(reconciledConversations)
        showToast(t('project.removeFailed'))
        return false
      }

      const removedConversationIds = new Set(
        conversationsRef.current
          .filter((conversation) => conversation.projectId === projectId)
          .map((conversation) => conversation.id)
      )
      for (const conversationId of removedConversationIds) {
        conversationScrollPositionsRef.current.delete(conversationId)
        pendingConversationSavesRef.current.delete(conversationId)
        pendingMessageUpsertsRef.current.delete(conversationId)
      }
      for (const [key, pendingSave] of pendingMessageSavesRef.current) {
        if (removedConversationIds.has(pendingSave.conversationId)) {
          pendingMessageSavesRef.current.delete(key)
        }
      }

      setConversationsWithRef((currentConversations) =>
        currentConversations.filter((conversation) => !removedConversationIds.has(conversation.id))
      )
      setDrafts((currentDrafts) =>
        Object.fromEntries(
          Object.entries(currentDrafts)
            .filter(([scopeId]) => !removedConversationIds.has(scopeId))
            .map(([scopeId, draft]) => [
              scopeId,
              draft.projectId === projectId ? createComposerDraft() : draft
            ])
        )
      )
      setUiPreferences((currentPreferences) => {
        const sidebarProjectOrder = currentPreferences.sidebarProjectOrder.filter(
          (orderedProjectId) => orderedProjectId !== projectId
        )
        if (sidebarProjectOrder.length === currentPreferences.sidebarProjectOrder.length) {
          return currentPreferences
        }

        const nextPreferences = {
          ...currentPreferences,
          sidebarProjectOrder,
          updatedAt: Date.now()
        }
        void saveUiPreferences(nextPreferences)
        return nextPreferences
      })

      if (
        activeConversationIdRef.current &&
        removedConversationIds.has(activeConversationIdRef.current)
      ) {
        activeConversationIdRef.current = null
        setActiveConversationId(null)
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)
      }
      return true
    },
    [
      cleanupRunBinding,
      deleteProject,
      enqueueChatMessageStateSave,
      setConversationsWithRef,
      showToast,
      t,
      waitForConversationSaves,
      waitForMessageStateSaves,
      waitForMessageUpserts
    ]
  )

  const stopActiveGeneration = useCallback(() => {
    if (!activeConversationId) return
    const stoppedAt = Date.now()
    const activeConversationSnapshot = conversations.find(
      (conversation) => conversation.id === activeConversationId
    )
    const pendingMessage = [...(activeConversationSnapshot?.messages ?? [])]
      .reverse()
      .find((message) => message.role === 'assistant' && message.status === 'pending')

    if (!pendingMessage) return
    cancelledPendingMessageIdsRef.current.add(pendingMessage.id)

    if (pendingMessage.agentRun?.runId) {
      cancelledRunIdsRef.current.add(pendingMessage.agentRun.runId)
      cancelBackendAgentRun(pendingMessage.agentRun.runId)
      cleanupRunBinding(pendingMessage.agentRun.runId)
    }

    updateAssistantMessage(
      activeConversationId,
      pendingMessage.id,
      (message) => {
        const currentRun = ensureAgentRun(
          message.agentRun,
          pendingMessage.agentRun?.runId ?? null,
          'cancelled'
        )
        return {
          ...message,
          content:
            message.content && message.content !== THINKING_PLACEHOLDER ? message.content : '',
          status: 'sent',
          agentRun: settleAgentRunToolActivities(
            {
              ...currentRun,
              completedAt: stoppedAt,
              todo: undefined
            },
            'cancelled',
            stoppedAt
          )
        }
      },
      { touchConversation: true }
    )
  }, [
    activeConversationId,
    cancelBackendAgentRun,
    cleanupRunBinding,
    conversations,
    updateAssistantMessage
  ])

  const handleApproveAgentAction = useCallback(
    (messageId: string, action: AgentProposedAction) => {
      if (!activeConversationId) return
      const conversationId = activeConversationId
      const actionId = getAgentActionId(action)
      void approveAgentAction(actionId)
        .then((execution) => {
          updateAssistantMessage(
            conversationId,
            messageId,
            (message) => applyAgentActionExecutionToChatMessage(message, execution),
            { touchConversation: true }
          )
        })
        .catch((error) => {
          console.error('Failed to approve agent action', error)
        })
      updateAssistantMessage(
        conversationId,
        messageId,
        (message) => applyAgentActionDecisionToChatMessage(message, action, 'approved'),
        { touchConversation: true }
      )
    },
    [activeConversationId, updateAssistantMessage]
  )

  const handleRejectAgentAction = useCallback(
    (messageId: string, action: AgentProposedAction, message?: string) => {
      if (!activeConversationId) return
      const conversationId = activeConversationId
      const actionId = getAgentActionId(action)
      void rejectAgentAction(actionId, message)
        .then((execution) => {
          updateAssistantMessage(
            conversationId,
            messageId,
            (currentMessage) => applyAgentActionExecutionToChatMessage(currentMessage, execution),
            { touchConversation: true }
          )
        })
        .catch((error) => {
          console.error('Failed to reject agent action', error)
        })
      updateAssistantMessage(
        conversationId,
        messageId,
        (currentMessage) =>
          applyAgentActionDecisionToChatMessage(currentMessage, action, 'rejected', message),
        { touchConversation: true }
      )
    },
    [activeConversationId, updateAssistantMessage]
  )

  const handleCancelAgentAction = useCallback(
    (messageId: string, action: AgentProposedAction) => {
      if (!activeConversationId) return
      const actionId = getAgentActionId(action)
      void cancelAgentAction(actionId).catch((error) => {
        console.error('Failed to cancel agent action', error)
      })
      updateAssistantMessage(
        activeConversationId,
        messageId,
        (message) => applyAgentActionDecisionToChatMessage(message, action, 'rejected'),
        { touchConversation: true }
      )
    },
    [activeConversationId, updateAssistantMessage]
  )

  const openSettings = useCallback((initialPage: SettingsPageId = 'general') => {
    setSettingsInitialPage(initialPage)
    setView('settings')
  }, [])
  const appWindowMaximized = appWindowState.isFullScreen || appWindowState.isMaximized

  if (view === 'settings') {
    return (
      <AppShellSettingsView
        conversations={conversations}
        initialPage={settingsInitialPage}
        onBack={() => setView('workspace')}
        onConversationPatch={patchConversation}
        onConversationsChange={setConversationsWithRef}
        onRemoveProject={removeProject}
        onUiPreferencesChange={updateUiPreferences}
        projects={projects}
        uiPreferences={uiPreferences}
      />
    )
  }

  return (
    <div
      ref={shellRef}
      className="app-shell"
      data-left-open={leftOpen ? 'true' : 'false'}
      data-native-font-smoothing={
        SUPPORTS_NATIVE_FONT_SMOOTHING && uiPreferences.nativeFontSmoothing ? 'true' : undefined
      }
      data-right-maximized={rightMaximized ? 'true' : undefined}
      data-right-open={rightOpen ? 'true' : 'false'}
      data-translucent-sidebar={uiPreferences.translucentSidebar ? 'true' : undefined}
      data-window-maximized={appWindowMaximized ? 'true' : undefined}
      style={getAppShellPanelStyle(leftOpen, leftWidth, rightOpen, rightWidth, uiPreferences)}
    >
      <header className="window-toolbar" data-drag-region />

      <aside className="side-panel side-panel--left">
        <div className="side-panel__surface">
          <LeftSidebar
            activeConversationId={activeConversationId}
            conversations={conversations}
            projects={projects}
            uiPreferences={uiPreferences}
            onArchiveAllProjectConversations={() => {
              const projectIds = new Set(projects.map((project) => project.id))
              archiveConversations(
                (conversation) =>
                  Boolean(conversation.projectId) && projectIds.has(conversation.projectId!)
              )
            }}
            onArchiveAllRootConversations={() => {
              const projectIds = new Set(projects.map((project) => project.id))
              archiveConversations(
                (conversation) => !conversation.projectId || !projectIds.has(conversation.projectId)
              )
            }}
            onArchiveConversation={(conversationId) =>
              patchConversation(conversationId, { archivedAt: Date.now() })
            }
            onArchiveProjectConversations={(projectId) =>
              archiveConversations((conversation) => conversation.projectId === projectId)
            }
            onMarkConversationUnread={(conversationId) =>
              patchConversation(conversationId, { unreadAt: Date.now() })
            }
            onNewConversation={(projectId = null) => {
              activeConversationIdRef.current = null
              setActiveConversationId(null)
              setScrollTargetMessageId(null)
              setActiveConversationInitialScrollTop(null)
              updateDraft(NEW_CONVERSATION_DRAFT_ID, createComposerDraft({ projectId }))
            }}
            onOpenSettings={() => openSettings('general')}
            onRemoveProject={removeProject}
            onRenameConversation={(conversationId, title) =>
              patchConversation(conversationId, { title })
            }
            onRenameProject={renameProject}
            onSelectConversation={selectConversation}
            onShowProjectInFolder={showProjectInFolder}
            onTogglePinConversation={(conversationId) => {
              const conversation = conversations.find(
                (candidate) => candidate.id === conversationId
              )
              patchConversation(conversationId, {
                pinnedAt: conversation?.pinnedAt ? null : Date.now()
              })
            }}
            onTogglePinProject={togglePinProject}
            onUiPreferencesChange={updateUiPreferences}
          />
        </div>
      </aside>

      {leftOpen && <ResizeHandle side="left" onResize={(deltaX) => resizeSide('left', deltaX)} />}

      <main className="main-panel" aria-label={t('app.mainWorkspace')}>
        <MainPanelToolbar
          hasUnreadConversations={hasUnreadConversations}
          leftOpen={leftOpen}
          onToggleLeftSidebar={toggleLeftSidebar}
          onToggleRightSidebar={toggleRightSidebar}
          rightOpen={rightOpen}
          t={t}
          title={activeConversation?.title}
        />

        <div className="main-panel__surface">
          {activeConversation ? (
            <ChatConversationPage
              composerDraft={activeDraft}
              conversation={activeConversation}
              contextWindowIndicatorEnabled={contextWindowIndicatorEnabled}
              contextWindowSnapshot={activeContextWindowSnapshot}
              editSelectedModelAvailable={Boolean(activeDraftSelectedModel)}
              editSelectedModelSupportsImage={Boolean(activeDraftSelectedModel?.supportsImage)}
              initialScrollTop={activeConversationInitialScrollTop}
              permissionModeAvailability={permissionModeAvailability}
              scrollToBottomSignal={conversationScrollToBottomSignal}
              scrollTargetMessageId={scrollTargetMessageId}
              showTokenUsageDetails={uiPreferences.showTokenUsageDetails}
              onApproveAgentAction={handleApproveAgentAction}
              onCancelAgentAction={handleCancelAgentAction}
              onComposerDraftChange={(draft) => updateDraft(activeConversation.id, draft)}
              onEditLastUserMessage={submitEditedLastUserMessage}
              onContinueInNewTask={(messageId) =>
                continueInNewTask(activeConversation.id, messageId)
              }
              onMessageUiStateChange={(messageId, uiState: ChatMessageUiState | undefined) => {
                const currentMessage = activeConversation.messages.find(
                  (message) => message.id === messageId
                )
                const messageToSave: ChatMessage | null = currentMessage
                  ? { ...currentMessage, uiState }
                  : null
                setConversationsWithRef((currentConversations) =>
                  currentConversations.map((conversation) =>
                    conversation.id === activeConversation.id
                      ? {
                          ...conversation,
                          messages: conversation.messages.map((message) =>
                            message.id === messageId ? (messageToSave ?? message) : message
                          )
                        }
                      : conversation
                  )
                )
                if (messageToSave) {
                  enqueueChatMessageStateSave(activeConversation.id, messageToSave)
                }
              }}
              onRejectAgentAction={handleRejectAgentAction}
              onScrollPositionChange={rememberConversationScrollPosition}
              onStopGenerating={stopActiveGeneration}
              onSubmitMessage={submitMessage}
            />
          ) : (
            <NewConversationPage
              contextWindowIndicatorEnabled={contextWindowIndicatorEnabled}
              contextWindowSnapshot={activeContextWindowSnapshot}
              draft={activeDraft}
              permissionModeAvailability={permissionModeAvailability}
              onDraftChange={(draft) => updateDraft(NEW_CONVERSATION_DRAFT_ID, draft)}
              onSubmitMessage={submitMessage}
            />
          )}
        </div>
      </main>

      {rightOpen && !rightMaximized && (
        <ResizeHandle side="right" onResize={(deltaX) => resizeSide('right', deltaX)} />
      )}

      <aside className="side-panel side-panel--right">
        <RightSidebar
          isMaximized={rightMaximized}
          workspaceKey={rightSidebarTerminalProject?.id}
          workspaceName={rightSidebarTerminalProject?.name}
          workspacePath={rightSidebarTerminalPath}
          onToggleMaximized={toggleRightSidebarMaximized}
          maximizedToolbarControls={
            rightMaximized ? (
              <MaximizedSidebarControls
                hasUnreadConversations={hasUnreadConversations}
                leftOpen={leftOpen}
                onToggleLeftSidebar={toggleLeftSidebar}
                onToggleRightSidebar={toggleRightSidebar}
                rightOpen={rightOpen}
                t={t}
              />
            ) : null
          }
        />
      </aside>
    </div>
  )
}
