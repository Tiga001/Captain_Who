// Renderer UI.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { AgentEvent, AgentProposedAction } from '@mycopilot/protocol'
import { ResizeHandle } from '../components/layout/ResizeHandle'
import { LeftSidebar } from '../components/sidebar/LeftSidebar'
import { RightSidebar } from '../components/sidebar/RightSidebar'
import { useProjectSettings } from '../config/ProjectSettingsProvider'
import { useFrontendConfig } from '../config/FrontendConfigProvider'
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
  listPendingAgentActions,
  onAgentEvent,
  rejectAgentAction,
  startConversationTurn
} from '../features/agent/agentClient'
import { resolveChatPermissions } from '../features/chat/chatPermissions'
import {
  defaultUiPreferences,
  loadComposerDrafts,
  loadConversations,
  loadUiPreferences,
  saveChatMessageState,
  saveComposerDraft,
  saveConversation,
  saveConversationMeta,
  saveUiPreferences
} from '../features/storage/storageClient'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { NEW_CONVERSATION_DRAFT_ID, THINKING_PLACEHOLDER } from './appConstants'
import type { ActiveRunBinding } from './appTypes'
import { getAgentActionId } from './agentActionUtils'
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

export function AppShell() {
  const { t } = useFrontendConfig()
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
  const [settingsInitialPage, setSettingsInitialPage] = useState<SettingsPageId>('general')
  const [uiPreferences, setUiPreferences] = useState<UiPreferencesSnapshot>(() =>
    defaultUiPreferences()
  )
  const [conversations, setConversations] = useState<ChatConversation[]>([])
  const conversationsRef = useRef<ChatConversation[]>([])
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null)
  const activeConversationIdRef = useRef<string | null>(null)
  const activeRunBindingsRef = useRef<Map<string, ActiveRunBinding>>(new Map())
  const bufferedAgentEventsRef = useRef<Map<string, AgentEvent[]>>(new Map())
  const pendingMessageDeltasRef = useRef<Map<string, PendingMessageDelta>>(new Map())
  const messageSaveQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingMessageSavesRef = useRef<Map<string, PendingMessageSave>>(new Map())
  const pendingActionsHydratedRef = useRef(false)
  const cancelledPendingMessageIdsRef = useRef<Set<string>>(new Set())
  const cancelledRunIdsRef = useRef<Set<string>>(new Set())
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
  const permissionModeAvailability = getPermissionModeAvailability(uiPreferences)
  const hasUnreadConversations = conversations.some(
    (conversation) => !conversation.archivedAt && Boolean(conversation.unreadAt)
  )

  useEffect(() => {
    activeConversationIdRef.current = activeConversationId
  }, [activeConversationId])

  useEffect(() => {
    conversationsRef.current = conversations
  }, [conversations])

  useEffect(() => {
    let cancelled = false
    void Promise.all([loadUiPreferences(), loadConversations(), loadComposerDrafts()]).then(
      ([preferences, storedConversations, storedDrafts]) => {
        if (cancelled) return
        setUiPreferences(preferences)
        conversationsRef.current = storedConversations
        setConversations(storedConversations)
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
  }, [])

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
    []
  )

  const updateAssistantMessage = useCallback(
    (
      conversationId: string,
      messageId: string,
      updater: (message: ChatMessage) => ChatMessage,
      options: { touchConversation?: boolean } = {}
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

      conversationsRef.current = nextConversations
      setConversations(nextConversations)

      if (messageToSave) {
        enqueueChatMessageStateSave(conversationId, messageToSave)
      }
      if (options.touchConversation && conversationMetaToSave) {
        void saveConversationMeta(conversationMetaToSave)
      }
    },
    [enqueueChatMessageStateSave]
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
            conversationsRef.current = nextConversations
            setConversations(nextConversations)
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
    [bufferMessageDelta, cleanupRunBinding, flushPendingMessageDelta, updateAssistantMessage]
  )

  useEffect(() => {
    return onAgentEvent((agentEvent) => {
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
  }, [handleBoundAgentEvent])

  useEffect(() => {
    return () => {
      for (const pendingDelta of pendingMessageDeltasRef.current.values()) {
        window.clearTimeout(pendingDelta.timerId)
      }
      pendingMessageDeltasRef.current.clear()
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
        conversationsRef.current = nextConversations
        setConversations(nextConversations)

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
      handleBoundAgentEvent,
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
      const title = createConversationTitle(message)
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

      conversationsRef.current = nextConversations
      setConversations(nextConversations)
      void saveConversation(conversationToSave)
      activeConversationIdRef.current = conversationId
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
    [activeConversation, requestAssistantResponse, updateDraft]
  )

  const selectConversation = useCallback((conversationId: string) => {
    activeConversationIdRef.current = conversationId

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
      conversationsRef.current = nextConversations
      setConversations(nextConversations)
      void saveConversationMeta(conversationToSave)
    }

    setActiveConversationId(conversationId)
  }, [])

  const patchConversation = useCallback(
    (conversationId: string, patch: Partial<ChatConversation>) => {
      let nextConversation: ChatConversation | null = null
      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation

        nextConversation = { ...conversation, ...patch }
        return nextConversation
      })

      if (nextConversation) {
        conversationsRef.current = nextConversations
        setConversations(nextConversations)
        void saveConversationMeta(nextConversation)
      }
    },
    []
  )

  const archiveConversations = useCallback(
    (predicate: (conversation: ChatConversation) => boolean) => {
      const archivedAt = Date.now()
      setConversations((currentConversations) =>
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
    []
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
              completedAt: stoppedAt
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

  if (view === 'settings') {
    return (
      <AppShellSettingsView
        conversations={conversations}
        initialPage={settingsInitialPage}
        onBack={() => setView('workspace')}
        onConversationPatch={patchConversation}
        onConversationsChange={setConversations}
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
      style={getAppShellPanelStyle(leftOpen, leftWidth, rightOpen, rightWidth)}
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
              updateDraft(NEW_CONVERSATION_DRAFT_ID, createComposerDraft({ projectId }))
            }}
            onOpenSettings={() => openSettings('general')}
            onRemoveProject={deleteProject}
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
        />

        <div className="main-panel__surface">
          {activeConversation ? (
            <ChatConversationPage
              composerDraft={activeDraft}
              conversation={activeConversation}
              permissionModeAvailability={permissionModeAvailability}
              showTokenUsageDetails={uiPreferences.showTokenUsageDetails}
              onApproveAgentAction={handleApproveAgentAction}
              onCancelAgentAction={handleCancelAgentAction}
              onComposerDraftChange={(draft) => updateDraft(activeConversation.id, draft)}
              onMessageUiStateChange={(messageId, uiState: ChatMessageUiState | undefined) => {
                const currentMessage = activeConversation.messages.find(
                  (message) => message.id === messageId
                )
                const messageToSave: ChatMessage | null = currentMessage
                  ? { ...currentMessage, uiState }
                  : null
                setConversations((currentConversations) =>
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
              onStopGenerating={stopActiveGeneration}
              onSubmitMessage={submitMessage}
            />
          ) : (
            <NewConversationPage
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
          workspaceName={projects[0]?.name ?? 'MyCopilot'}
          workspacePath={projects[0]?.path}
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
