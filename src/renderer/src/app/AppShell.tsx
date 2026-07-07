// Renderer UI.
import { useCallback, useEffect, useMemo, useState } from 'react'
import type { CSSProperties } from 'react'
import type { AppVersionResponse, CorePingResponse } from '@mycopilot/protocol'
import { ResizeHandle } from '../components/layout/ResizeHandle'
import { LeftSidebar } from '../components/sidebar/LeftSidebar'
import { RightSidebar } from '../components/sidebar/RightSidebar'
import { useProjectSettings } from '../config/ProjectSettingsProvider'
import { useFrontendConfig } from '../config/FrontendConfigProvider'
import { ChatConversationPage } from '../features/chat/ChatConversationPage'
import { NewConversationPage } from '../features/chat/NewConversationPage'
import { SettingsPage } from '../features/settings/SettingsPage'
import type { SettingsPageId } from '../features/settings/SettingsPage'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatMessageUiState,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import {
  defaultUiPreferences,
  deleteStoredConversation,
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
import { NEW_CONVERSATION_DRAFT_ID } from './appConstants'
import {
  createAssistantMessage,
  createComposerDraft,
  createConversationTitle,
  createId,
  createUserMessage
} from './chatMessageFactory'
import { hostClient } from '../host/hostClient'
import { isMacOS } from '../lib/platform'
import { useShellLayout } from './useShellLayout'
import './AppShell.css'

type AppView = 'workspace' | 'settings'
const SUPPORTS_NATIVE_FONT_SMOOTHING = isMacOS()

function SidebarToggleIcon({ open, side }: { open: boolean; side: 'left' | 'right' }) {
  return (
    <span
      aria-hidden="true"
      className="panel-toggle__icon"
      data-open={open ? 'true' : 'false'}
      data-side={side}
    />
  )
}

function CoreStatusWidget() {
  const [ping, setPing] = useState<CorePingResponse | null>(null)
  const [version, setVersion] = useState<AppVersionResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [isChecking, setIsChecking] = useState(false)

  const checkCore = useCallback(async () => {
    setIsChecking(true)
    setError(null)

    try {
      const [nextPing, nextVersion] = await Promise.all([
        hostClient.core.ping({ message: 'renderer-shell' }),
        hostClient.app.getVersion()
      ])
      setPing(nextPing)
      setVersion(nextVersion)
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : 'core-server error')
    } finally {
      setIsChecking(false)
    }
  }, [])

  useEffect(() => {
    void checkCore()
  }, [checkCore])

  return (
    <section className="core-status" aria-label="Core server status">
      <div>
        <span className="core-status__label">core-server</span>
        <strong data-state={error ? 'error' : ping ? 'ready' : 'pending'}>
          {error ? 'error' : ping ? 'ready' : 'starting'}
        </strong>
      </div>
      <div>
        <span className="core-status__label">ping</span>
        <strong>{ping?.message ?? 'pending'}</strong>
      </div>
      <div>
        <span className="core-status__label">version</span>
        <strong>{version?.version ?? '0.1.0'}</strong>
      </div>
      <button type="button" onClick={checkCore} disabled={isChecking}>
        {isChecking ? 'Checking' : 'Ping'}
      </button>
      {error && <p>{error}</p>}
    </section>
  )
}

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
  const [view, setView] = useState<AppView>('workspace')
  const [settingsInitialPage, setSettingsInitialPage] = useState<SettingsPageId>('general')
  const [uiPreferences, setUiPreferences] = useState<UiPreferencesSnapshot>(() =>
    defaultUiPreferences()
  )
  const [conversations, setConversations] = useState<ChatConversation[]>([])
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null)
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
  const permissionModeAvailability = {
    custom: uiPreferences.customPermissionEnabled,
    full: uiPreferences.fullPermissionEnabled
  }
  const hasUnreadConversations = conversations.some(
    (conversation) => !conversation.archivedAt && Boolean(conversation.unreadAt)
  )

  useEffect(() => {
    let cancelled = false
    void Promise.all([loadUiPreferences(), loadConversations(), loadComposerDrafts()]).then(
      ([preferences, storedConversations, storedDrafts]) => {
        if (cancelled) return
        setUiPreferences(preferences)
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

  const submitMessage = useCallback(
    (message: string, options: ChatSubmitOptions) => {
      const now = Date.now()
      const conversationId = activeConversation?.id ?? createId('conversation')
      const userMessage = createUserMessage(message, options.attachments ?? [])
      const assistantMessage = createAssistantMessage(
        'This UI shell is connected to the Rust core-server ping path. Full agent runs are reserved for the next integration phase.'
      )
      const conversationToSave: ChatConversation = activeConversation
        ? {
            ...activeConversation,
            messages: [...activeConversation.messages, userMessage, assistantMessage],
            updatedAt: now
          }
        : {
            id: conversationId,
            projectId: options.projectId,
            modelId: options.modelId,
            title: createConversationTitle(message),
            messages: [userMessage, assistantMessage],
            createdAt: now,
            updatedAt: now,
            pinnedAt: null,
            archivedAt: null,
            unreadAt: null
          }

      setConversations((currentConversations) => {
        const existingConversation = currentConversations.find(
          (conversation) => conversation.id === conversationId
        )
        if (existingConversation) {
          return currentConversations.map((conversation) =>
            conversation.id === conversationId ? conversationToSave : conversation
          )
        }
        return [conversationToSave, ...currentConversations]
      })
      void saveConversation(conversationToSave)
      setActiveConversationId(conversationId)
      updateDraft(
        conversationId,
        createComposerDraft({ modelId: options.modelId, projectId: options.projectId })
      )
    },
    [activeConversation, updateDraft]
  )

  const patchConversation = useCallback(
    (conversationId: string, patch: Partial<ChatConversation>) => {
      const conversationToSave = conversations.find(
        (conversation) => conversation.id === conversationId
      )
      const nextConversation = conversationToSave ? { ...conversationToSave, ...patch } : null
      setConversations((currentConversations) =>
        currentConversations.map((conversation) => {
          if (conversation.id !== conversationId) return conversation
          return nextConversation ?? conversation
        })
      )
      if (nextConversation) {
        void saveConversationMeta(nextConversation)
      }
    },
    [conversations]
  )

  const openSettings = useCallback((initialPage: SettingsPageId = 'general') => {
    setSettingsInitialPage(initialPage)
    setView('settings')
  }, [])

  if (view === 'settings') {
    return (
      <SettingsPage
        conversations={conversations}
        initialPage={settingsInitialPage}
        projects={projects}
        uiPreferences={uiPreferences}
        onBack={() => setView('workspace')}
        onDeleteAllArchivedConversations={() =>
          setConversations((currentConversations) => {
            currentConversations
              .filter((conversation) => conversation.archivedAt)
              .forEach((conversation) => void deleteStoredConversation(conversation.id))
            return currentConversations.filter((conversation) => !conversation.archivedAt)
          })
        }
        onDeleteConversation={(conversationId) => {
          setConversations((currentConversations) =>
            currentConversations.filter((conversation) => conversation.id !== conversationId)
          )
          void deleteStoredConversation(conversationId)
        }}
        onUnarchiveConversation={(conversationId) =>
          patchConversation(conversationId, { archivedAt: null })
        }
        onUiPreferencesChange={updateUiPreferences}
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
      style={
        {
          '--left-panel-width': `${leftOpen ? leftWidth : 0}px`,
          '--right-panel-width': `${rightOpen ? rightWidth : 0}px`
        } as CSSProperties
      }
    >
      <header className="window-toolbar" data-drag-region />

      <aside className="side-panel side-panel--left">
        <div className="side-panel__surface">
          <LeftSidebar
            activeConversationId={activeConversationId}
            conversations={conversations}
            projects={projects}
            uiPreferences={uiPreferences}
            onArchiveAllProjectConversations={() => undefined}
            onArchiveAllRootConversations={() => undefined}
            onArchiveConversation={(conversationId) =>
              patchConversation(conversationId, { archivedAt: Date.now() })
            }
            onArchiveProjectConversations={() => undefined}
            onMarkConversationUnread={(conversationId) =>
              patchConversation(conversationId, { unreadAt: Date.now() })
            }
            onNewConversation={(projectId = null) => {
              setActiveConversationId(null)
              updateDraft(NEW_CONVERSATION_DRAFT_ID, createComposerDraft({ projectId }))
            }}
            onOpenSettings={() => openSettings('general')}
            onRemoveProject={deleteProject}
            onRenameConversation={(conversationId, title) =>
              patchConversation(conversationId, { title })
            }
            onRenameProject={renameProject}
            onSelectConversation={setActiveConversationId}
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
        <div className="main-panel__toolbar" data-drag-region>
          <button
            className="panel-toggle panel-toggle--left"
            data-has-unread={!leftOpen && hasUnreadConversations ? 'true' : undefined}
            type="button"
            aria-label={leftOpen ? t('app.collapseLeftSidebar') : t('app.expandLeftSidebar')}
            aria-pressed={leftOpen}
            onClick={toggleLeftSidebar}
          >
            <SidebarToggleIcon open={leftOpen} side="left" />
          </button>
          <CoreStatusWidget />
          <button
            className="panel-toggle panel-toggle--right"
            type="button"
            aria-label={rightOpen ? t('app.collapseRightSidebar') : t('app.expandRightSidebar')}
            aria-pressed={rightOpen}
            onClick={toggleRightSidebar}
          >
            <SidebarToggleIcon open={rightOpen} side="right" />
          </button>
        </div>

        <div className="main-panel__surface">
          {activeConversation ? (
            <ChatConversationPage
              composerDraft={activeDraft}
              conversation={activeConversation}
              permissionModeAvailability={permissionModeAvailability}
              showTokenUsageDetails={uiPreferences.showTokenUsageDetails}
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
                  void saveChatMessageState(activeConversation.id, messageToSave)
                }
              }}
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
              <>
                <button
                  className="right-sidebar__icon-button"
                  type="button"
                  aria-label={leftOpen ? t('app.collapseLeftSidebar') : t('app.expandLeftSidebar')}
                  aria-pressed={leftOpen}
                  onClick={toggleLeftSidebar}
                  title={leftOpen ? t('app.collapseLeftSidebar') : t('app.expandLeftSidebar')}
                >
                  <SidebarToggleIcon open={leftOpen} side="left" />
                </button>
                <button
                  className="right-sidebar__icon-button"
                  type="button"
                  aria-label={
                    rightOpen ? t('app.collapseRightSidebar') : t('app.expandRightSidebar')
                  }
                  aria-pressed={rightOpen}
                  onClick={toggleRightSidebar}
                  title={rightOpen ? t('app.collapseRightSidebar') : t('app.expandRightSidebar')}
                >
                  <SidebarToggleIcon open={rightOpen} side="right" />
                </button>
              </>
            ) : null
          }
        />
      </aside>
    </div>
  )
}
