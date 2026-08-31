import type {
  AgentEvent,
  AgentObserverConversation,
  AgentObserverEventEnvelope,
  AgentTreeSnapshot,
  CollaborationApprovalList,
  CollaborationEventEnvelope,
  CollaborationEventsPage
} from '@mycopilot/protocol'
import {
  parseAgentObserverConversation,
  parseAgentObserverEventEnvelope,
  parseAgentTreeSnapshot,
  parseCollaborationApprovalDecisionResult,
  parseCollaborationApprovalList,
  parseCollaborationEventEnvelope,
  parseCollaborationEventsPage
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import scenarioFixture from '../../../../../packages/protocol/fixtures/agent-collaboration-round5-scenario-v1.json'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { enUSTranslations } from '../../config/frontendTranslations.enUS'

interface CollaborationScenarioState {
  agentEventListeners: Set<(event: AgentEvent) => void>
  approvalList: CollaborationApprovalList | null
  collaborationEventListeners: Set<(event: CollaborationEventEnvelope) => void>
  currentTree: AgentTreeSnapshot | null
  decisionInputs: Array<{
    approvalId: string
    decision: 'approve' | 'reject' | 'cancel'
    message: string | null
    rootConversationId: string
  }>
  eventPage: CollaborationEventsPage | null
  observers: Map<string, AgentObserverConversation>
  observerEventListeners: Set<(event: AgentObserverEventEnvelope) => void>
  observerLoadRequests: string[]
  resyncListeners: Set<() => void>
}

const scenarioState = vi.hoisted<CollaborationScenarioState>(() => ({
  agentEventListeners: new Set(),
  approvalList: null,
  collaborationEventListeners: new Set(),
  currentTree: null,
  decisionInputs: [],
  eventPage: null,
  observers: new Map(),
  observerEventListeners: new Set(),
  observerLoadRequests: [],
  resyncListeners: new Set()
}))

vi.mock('../../config/FrontendConfigProvider', () => {
  const t = (key: keyof typeof enUSTranslations) => enUSTranslations[key]
  const config = { language: 'en-US', t }
  return { useFrontendConfig: () => config }
})

vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [
      {
        id: 'model-1',
        displayName: 'Model One',
        supportsImage: true,
        inputPrice: '0',
        outputPrice: '0',
        enabled: true
      },
      {
        id: 'model-2',
        displayName: 'Model Two',
        supportsImage: true,
        inputPrice: '0',
        outputPrice: '0',
        enabled: true
      }
    ],
    models: []
  })
}))

vi.mock('../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({
    projects: [{ id: 'project-a', name: 'Project A', path: '/workspace/a', createdAt: 1 }],
    deleteProject: vi.fn(),
    renameProject: vi.fn(),
    showProjectInFolder: vi.fn(),
    togglePinProject: vi.fn()
  })
}))

vi.mock('../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))

vi.mock('../useShellLayout', () => ({
  useShellLayout: () => ({
    commitSidebarResize: vi.fn(),
    leftResizeMetrics: { maximum: 420, minimum: 220, width: 0 },
    leftOpen: false,
    leftWidth: 0,
    openRightSidebar: vi.fn(),
    rightMaximized: false,
    rightOpen: true,
    rightResizeMetrics: { maximum: 1200, minimum: 280, width: 360 },
    rightWidth: 360,
    shellRef: { current: null },
    toggleLeftSidebar: vi.fn(),
    toggleRightSidebar: vi.fn(),
    toggleRightSidebarMaximized: vi.fn()
  })
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    app: {
      getWindowState: vi.fn().mockResolvedValue({ isFullScreen: false, isMaximized: false }),
      onWindowStateChange: vi.fn(() => () => undefined)
    },
    git: {
      getTurnDiffSummaries: vi.fn().mockResolvedValue({ summaries: [] })
    },
    agent: {
      getCollaborationTree: vi.fn(
        async ({ rootConversationId }: { rootConversationId: string }) => ({
          ok: true,
          value:
            scenarioState.currentTree?.rootConversationId === rootConversationId
              ? { schemaVersion: 1, materialized: true, tree: scenarioState.currentTree }
              : { schemaVersion: 1, materialized: false, tree: null }
        })
      ),
      listCollaborationEvents: vi.fn(
        async ({
          afterSequence,
          limit,
          rootConversationId
        }: {
          afterSequence: number
          limit: number
          rootConversationId: string
        }) => {
          const page = scenarioState.eventPage
          const visibleSequence =
            scenarioState.currentTree?.rootConversationId === rootConversationId
              ? scenarioState.currentTree.lastSequence
              : 0
          if (!page || page.rootConversationId !== rootConversationId) {
            return {
              ok: true,
              value: {
                schemaVersion: 1,
                rootAgentId: scenarioState.currentTree?.rootAgentId ?? 'unmaterialized-root',
                rootConversationId,
                events: [],
                lastSequence: afterSequence,
                hasMore: false
              }
            }
          }
          const available = page.events.filter(
            (event) => event.sequence > afterSequence && event.sequence <= visibleSequence
          )
          const events = available.slice(0, limit)
          return {
            ok: true,
            value: {
              ...page,
              events,
              hasMore: available.length > events.length,
              lastSequence: visibleSequence
            }
          }
        }
      ),
      loadCollaborationObserverConversation: vi.fn(
        async ({
          conversationId,
          rootConversationId
        }: {
          conversationId: string
          rootConversationId: string
        }) => {
          scenarioState.observerLoadRequests.push(`${rootConversationId}:${conversationId}`)
          const observer = scenarioState.observers.get(conversationId) ?? null
          return {
            ok: true,
            value: observer?.rootConversationId === rootConversationId ? observer : null
          }
        }
      ),
      listCollaborationApprovals: vi.fn(
        async ({ rootConversationId }: { rootConversationId: string }) => ({
          ok: true,
          value:
            rootConversationId === 'conversation-root'
              ? scenarioState.approvalList
              : { schemaVersion: 1, approvals: [] }
        })
      ),
      decideCollaborationApproval: vi.fn(
        async (input: CollaborationScenarioState['decisionInputs'][number]) => {
          scenarioState.decisionInputs.push(input)
          return {
            ok: true,
            value: parseCollaborationApprovalDecisionResult(
              input.decision === 'approve'
                ? scenarioFixture.approvalDecisions.approve
                : scenarioFixture.approvalDecisions.reject
            )
          }
        }
      ),
      onCollaborationEvent: (listener: (event: CollaborationEventEnvelope) => void) => {
        scenarioState.collaborationEventListeners.add(listener)
        return () => scenarioState.collaborationEventListeners.delete(listener)
      },
      onCollaborationObserverEvent: (listener: (event: AgentObserverEventEnvelope) => void) => {
        scenarioState.observerEventListeners.add(listener)
        return () => scenarioState.observerEventListeners.delete(listener)
      },
      onCollaborationResync: (listener: () => void) => {
        scenarioState.resyncListeners.add(listener)
        return () => scenarioState.resyncListeners.delete(listener)
      }
    }
  }
}))

vi.mock('../../features/gitReview/useGitRepositoryCapability', () => ({
  useGitRepositoryCapability: () => ({ status: 'unavailable' })
}))

vi.mock('../../features/agent/agentClient', () => ({
  approveAgentAction: vi.fn(),
  cancelAgentAction: vi.fn(),
  cancelAgentRun: vi.fn().mockResolvedValue(true),
  getContextWindowSnapshot: vi.fn().mockResolvedValue({ snapshot: null }),
  getProviderTransitionStatus: vi.fn().mockResolvedValue({ operations: [] }),
  getAgentCommandSession: vi.fn(),
  getAgentFileChangeDiff: vi.fn(),
  getAgentFileChangeHistoryDiff: vi.fn(),
  listAgentCommandSessions: vi.fn().mockResolvedValue({ sessions: [] }),
  listPendingAgentActions: vi.fn().mockResolvedValue([]),
  onAgentEvent: (listener: (event: AgentEvent) => void) => {
    scenarioState.agentEventListeners.add(listener)
    return () => scenarioState.agentEventListeners.delete(listener)
  },
  onProviderTransition: vi.fn(() => () => undefined),
  preflightProviderTransition: vi.fn(),
  readAgentFileChange: vi.fn(),
  rejectAgentAction: vi.fn(),
  rewriteConversationTurn: vi.fn(),
  startConversationTurn: vi.fn(),
  startProviderTransition: vi.fn(),
  steerAgentRun: vi.fn()
}))

vi.mock('../../features/storage/storageClient', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../features/storage/storageClient')>()
  return {
    ...original,
    deleteChatMessages: vi.fn(),
    forkConversation: vi.fn(),
    loadComposerDrafts: vi.fn().mockResolvedValue({}),
    loadConversation: vi.fn(async (conversationId: string) => rootConversation(conversationId)),
    loadConversationMetas: vi
      .fn()
      .mockImplementation(async () => [
        rootConversation('conversation-root', false),
        rootConversation('conversation-other', false)
      ]),
    loadInputAttachments: vi.fn().mockResolvedValue([]),
    loadUiPreferences: vi.fn(async () => ({
      ...original.defaultUiPreferences(),
      showContextWindowUsage: false,
      showTokenUsageDetails: true
    })),
    saveChatMessageState: vi.fn(),
    saveChatMessageUiState: vi.fn(),
    saveComposerDraft: vi.fn(),
    saveComposerDraftMessage: vi.fn().mockResolvedValue(true),
    saveConversationMeta: vi.fn(),
    saveUiPreferences: vi.fn(),
    upsertChatMessages: vi.fn()
  }
})

vi.mock('../../components/layout/ResizeHandle', () => ({ ResizeHandle: () => null }))
vi.mock('../shell/sidebar/LeftSidebar', () => ({
  LeftSidebar: ({
    conversations,
    onSelectConversation
  }: {
    conversations: ChatConversation[]
    onSelectConversation: (conversationId: string) => void
  }) => (
    <nav aria-label="fixture conversations">
      {conversations.map((conversation) => (
        <button
          key={conversation.id}
          onClick={() => onSelectConversation(conversation.id)}
          type="button"
        >
          select-{conversation.id}
        </button>
      ))}
    </nav>
  )
}))
vi.mock('../AppShellSettingsView', () => ({ AppShellSettingsView: () => null }))
vi.mock('../../features/chat/NewConversationPage', () => ({
  NewConversationPage: () => <div data-testid="new-conversation" />
}))

const { AppShell } = await import('../AppShell')

const frozenRootCollaborationActivities = parseCollaborationEventsPage(
  scenarioFixture.settledEventPage
).events.flatMap((event) => {
  const activity = event.activity
  if (
    !activity ||
    activity.rootAnchorMessageId !== 'assistant-conversation-root' ||
    activity.rootTraceBoundarySequence === null
  ) {
    return []
  }

  return [
    {
      activityId: event.eventId,
      agentId: activity.agentId,
      occurredAt: event.occurredAt,
      rootAnchorMessageId: activity.rootAnchorMessageId,
      rootTraceBoundarySequence: activity.rootTraceBoundarySequence,
      runId: event.runId,
      semantic: activity.semantic,
      sequence: event.sequence,
      taskNameSnapshot: activity.taskNameSnapshot,
      turnId: event.turnId
    }
  ]
})

function rootConversation(conversationId: string, loaded = true): ChatConversation {
  return {
    id: conversationId,
    projectId: 'project-a',
    modelId: 'model-1',
    title: conversationId === 'conversation-root' ? 'Harness root' : 'Other root',
    messages: loaded
      ? [
          {
            id: `user-${conversationId}`,
            role: 'user',
            content: 'Use deterministic collaboration.',
            createdAt: 1,
            status: 'sent'
          },
          {
            id: `assistant-${conversationId}`,
            role: 'assistant',
            content: 'Working with child agents.',
            createdAt: 2,
            status: 'sent',
            uiState: { timelineCollapsed: false },
            agentRun: {
              runId: `run-${conversationId}`,
              status: 'completed',
              startedAt: 2,
              completedAt: 3,
              toolDefinitions: [],
              toolCalls: [],
              toolResults: [],
              approvals: [],
              fileChangeProposals: [],
              collaborationTimelineActivities:
                conversationId === 'conversation-root' ? frozenRootCollaborationActivities : [],
              timeline: [
                {
                  id: `trace-${conversationId}-delegation`,
                  type: 'message',
                  content: 'Delegating to child agents.',
                  traceSequence: 0
                }
              ]
            }
          }
        ]
      : [],
    messagesLoaded: loaded,
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  }
}

function resetScenario(): void {
  vi.spyOn(Date, 'now').mockReturnValue(1_720_000_004_000)
  scenarioState.currentTree = parseAgentTreeSnapshot(scenarioFixture.runningTree)
  scenarioState.eventPage = parseCollaborationEventsPage(scenarioFixture.settledEventPage)
  scenarioState.approvalList = parseCollaborationApprovalList(scenarioFixture.approvalList)
  scenarioState.observers = new Map(
    scenarioFixture.observerConversations.map((value) => {
      const observer = parseAgentObserverConversation(value)
      if (!observer) throw new Error('Round 5 observer fixture cannot be null')
      return [observer.conversationId, observer]
    })
  )
  scenarioState.decisionInputs.length = 0
  scenarioState.agentEventListeners.clear()
  scenarioState.collaborationEventListeners.clear()
  scenarioState.observerEventListeners.clear()
  scenarioState.observerLoadRequests.length = 0
  scenarioState.resyncListeners.clear()
  localStorage.clear()
  sessionStorage.clear()
}

function emitCollaborationEvent(event: CollaborationEventEnvelope): void {
  for (const listener of scenarioState.collaborationEventListeners) listener(event)
}

function emitObserverEvent(event: AgentObserverEventEnvelope): void {
  for (const listener of scenarioState.observerEventListeners) listener(event)
}

function requiredAgentCenterRow(container: HTMLElement, agentId: string): HTMLButtonElement {
  const row = container.querySelector<HTMLButtonElement>(
    `.agent-center__row[data-agent-id="${agentId}"]`
  )
  if (!row) throw new Error(`Missing Agent Center row for ${agentId}`)
  return row
}

async function captureStableScreenshot(element: Element, path: string): Promise<void> {
  const staticMotionStyle = document.createElement('style')
  staticMotionStyle.textContent = `
    *, *::before, *::after {
      animation: none !important;
      caret-color: transparent !important;
      transition: none !important;
    }
  `
  document.head.append(staticMotionStyle)
  try {
    await document.fonts.ready
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    })
    const first = await page.screenshot({ base64: true, element, path })
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    })
    const repeated = await page.screenshot({ element, save: false })
    expect(repeated).toBe(first.base64)
  } finally {
    staticMotionStyle.remove()
  }
}

beforeEach(resetScenario)

describe('AppShell deterministic collaboration scenario', () => {
  it('hydrates real Host DTOs into root activity, approval routing and read-only Agent Center', async () => {
    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'select-conversation-root' }).click()

    await expect.element(screen.getByTestId('collaboration-timeline')).toBeVisible()
    await expect
      .poll(() => screen.container.querySelectorAll('.collaboration-timeline__chip').length)
      .toBeGreaterThanOrEqual(2)
    expect(screen.container.querySelector('[data-semantic="waiting_approval"]')).toBeNull()
    expect(screen.container.querySelectorAll('[data-approval-id]')).toHaveLength(2)
    await captureStableScreenshot(
      screen.container.querySelector('.main-panel__surface') ?? screen.container,
      '__screenshots__/AppShellCollaborationScenario.browser.test.tsx/root-collaboration.png'
    )

    const timelineReviewChip = screen.container.querySelector<HTMLButtonElement>(
      '.collaboration-timeline__chip[data-agent-id="agent-review"]'
    )
    if (!timelineReviewChip) throw new Error('Missing review Agent timeline chip')
    await page.elementLocator(timelineReviewChip).click()
    await expect
      .poll(() =>
        screen.container.querySelector(
          '.agent-center__observer [data-conversation-id="conversation-review"]'
        )
      )
      .not.toBeNull()
    await expect
      .poll(() => scenarioState.observerLoadRequests)
      .toContain('conversation-root:conversation-review')

    const approveCard = screen.container.querySelector<HTMLElement>(
      '[data-approval-id="approval-approve"]'
    )
    const approveButton = approveCard?.querySelector<HTMLButtonElement>('[data-choice="primary"]')
    if (!approveButton) throw new Error('Missing approve button')
    await page.elementLocator(approveButton).click()
    await expect.poll(() => scenarioState.decisionInputs.length).toBe(1)

    // A fresh root projection (the same restart/remount path used below) exposes the second
    // pending action. The production list deliberately renders one actionable approval at a time.
    await screen.unmount()
    scenarioState.approvalList = {
      schemaVersion: 1,
      approvals: parseCollaborationApprovalList(scenarioFixture.approvalList).approvals.filter(
        (approval) => approval.approvalId === 'approval-reject'
      )
    }
    const rejectionScreen = await render(<AppShell />)
    await rejectionScreen.getByRole('button', { name: 'select-conversation-root' }).click()
    await expect.element(rejectionScreen.getByTestId('collaboration-timeline')).toBeVisible()
    const rejectButton = await rejectionScreen.getByRole('button', {
      name: enUSTranslations['agent.approval.dialog.reject']
    })
    await rejectButton.click()
    await expect.poll(() => scenarioState.decisionInputs.length).toBe(2)
    expect(scenarioState.decisionInputs).toEqual([
      {
        approvalId: 'approval-approve',
        decision: 'approve',
        message: null,
        rootConversationId: 'conversation-root'
      },
      {
        approvalId: 'approval-reject',
        decision: 'reject',
        message: null,
        rootConversationId: 'conversation-root'
      }
    ])

    const rootAgentListenerCount = scenarioState.agentEventListeners.size
    await expect.element(rejectionScreen.getByRole('button', { name: 'Subagents' })).toBeVisible()
    await page
      .elementLocator(rejectionScreen.getByRole('button', { name: 'Subagents' }).element())
      .click()
    await expect
      .poll(() =>
        rejectionScreen.container.querySelector('.agent-center__row[data-agent-id="agent-review"]')
      )
      .not.toBeNull()
    expect(rejectionScreen.container.textContent).toContain('Model One')
    expect(rejectionScreen.container.textContent).toContain('Model Two')
    await expect
      .poll(() => rejectionScreen.container.querySelector('.agent-center__observer'))
      .toBeNull()
    const securityRow = requiredAgentCenterRow(rejectionScreen.container, 'agent-review')
    securityRow.click()
    securityRow.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    await expect
      .poll(() => rejectionScreen.container.querySelector('.agent-center__observer'))
      .not.toBeNull()
    await expect
      .poll(() => scenarioState.observerLoadRequests)
      .toContain('conversation-root:conversation-review')
    expect(scenarioState.observerEventListeners.size).toBe(1)
    expect(scenarioState.agentEventListeners.size).toBe(rootAgentListenerCount + 1)
    await expect
      .poll(
        () => rejectionScreen.container.querySelector('.agent-center__state')?.textContent ?? ''
      )
      .not.toContain('Loading')
    await expect
      .element(rejectionScreen.getByText('Inspect the authentication boundary.'))
      .toBeVisible()
    expect(
      rejectionScreen.container.querySelector('[data-conversation-surface-mode="observer"]')
    ).not.toBeNull()
    expect(
      rejectionScreen.container.querySelector('.agent-center__observer .chat-composer')
    ).toBeNull()
    expect(rejectionScreen.container.querySelector('.agent-center__observer textarea')).toBeNull()
    expect(
      rejectionScreen.container.querySelector('.agent-center__observer .agent-approval-dialog')
    ).toBeNull()
    expect(rejectionScreen.container.textContent).toContain('Inspect the authentication boundary.')

    // Both children may stream at the same time. The authenticated Host envelope, rather than
    // whichever observer happens to be mounted, owns the run/message routing decision.
    emitObserverEvent(
      parseAgentObserverEventEnvelope({
        schemaVersion: 1,
        rootAgentId: 'agent-root',
        rootConversationId: 'conversation-root',
        agentId: 'agent-compatibility',
        conversationId: 'conversation-compatibility',
        runId: 'run-compatibility',
        assistantMessageId: 'assistant-compatibility',
        event: {
          type: 'message_delta',
          runId: 'run-compatibility',
          streamId: 'compatibility-stream',
          delta: 'compatibility-only live delta'
        }
      })
    )
    emitObserverEvent(
      parseAgentObserverEventEnvelope({
        schemaVersion: 1,
        rootAgentId: 'agent-root',
        rootConversationId: 'conversation-root',
        agentId: 'agent-review',
        conversationId: 'conversation-review',
        runId: 'run-review',
        assistantMessageId: 'assistant-review',
        event: {
          type: 'message_delta',
          runId: 'run-review',
          streamId: 'review-stream',
          delta: 'live review evidence'
        }
      })
    )
    await expect
      .poll(() => rejectionScreen.container.querySelector('.agent-center__observer')?.textContent)
      .toContain('live review evidence')
    expect(rejectionScreen.container.textContent).not.toContain('compatibility-only live delta')
    emitObserverEvent(
      parseAgentObserverEventEnvelope({
        schemaVersion: 1,
        rootAgentId: 'agent-root',
        rootConversationId: 'conversation-root',
        agentId: 'agent-review',
        conversationId: 'conversation-review',
        runId: 'run-review',
        assistantMessageId: 'assistant-review',
        event: {
          type: 'done',
          runId: 'run-review',
          success: true,
          status: 'completed',
          content: 'Review complete with no boundary violation.',
          usage: { inputTokens: 11, outputTokens: 7, totalTokens: 18 },
          proposedActions: []
        }
      })
    )
    await expect
      .poll(() =>
        rejectionScreen.container.querySelector<HTMLButtonElement>(
          '.agent-center__observer .chat-message__usage button'
        )
      )
      .not.toBeNull()
    // The fixture and terminal Host event both carry child-local Usage (18 tokens); the shared
    // Surface exposes it without aggregating the root or sibling Conversation.
    expect(
      scenarioState.observers
        .get('conversation-review')
        ?.messages.find((message) => message.messageId === 'assistant-review')?.agentRunJson
    ).toContain('"totalTokens":18')
    expect(rejectionScreen.container.querySelector('.agent-center__observer')).not.toBeNull()
    expect(rejectionScreen.container.textContent).toContain('Inspect the authentication boundary.')
    const usageButton = rejectionScreen.container.querySelector<HTMLButtonElement>(
      '.agent-center__observer .chat-message__usage button'
    )
    if (!usageButton) throw new Error('Missing observer Usage action')
    const agentCenter = rejectionScreen.container.querySelector<HTMLElement>(
      '.right-sidebar__page[data-active="true"] .agent-center--detail'
    )
    if (!agentCenter) throw new Error('Missing visible Agent Center detail')
    // Keep the desktop Shell's 520px minimum out of the 333px browser fixture by capturing a
    // detached visual copy of the already-rendered Agent Center. It carries the exact shared
    // ConversationSurface DOM and CSS, while avoiding unrelated panels underneath the crop.
    const visualCopy = agentCenter.cloneNode(true) as HTMLElement
    Object.assign(visualCopy.style, {
      background: 'white',
      height: '720px',
      left: '0',
      position: 'fixed',
      top: '0',
      width: '333px',
      zIndex: '2147483647'
    })
    const usageAction = visualCopy.querySelector<HTMLElement>('.chat-message__usage')
    const visualUsageButton = usageAction?.querySelector<HTMLButtonElement>('button')
    const usagePopover = usageAction?.querySelector<HTMLElement>('.chat-message__usage-popover')
    if (!usageAction || !visualUsageButton || !usagePopover) {
      throw new Error('Missing observer Usage popover')
    }
    usageAction.style.display = 'inline-flex'
    Object.assign(usagePopover.style, {
      bottom: 'auto',
      left: '0',
      opacity: '1',
      pointerEvents: 'auto',
      position: 'absolute',
      top: '28px',
      transform: 'translateY(0)'
    })
    document.body.append(visualCopy)
    await captureStableScreenshot(
      visualCopy,
      '__screenshots__/AppShellCollaborationScenario.browser.test.tsx/agent-observer.png'
    )
    visualCopy.remove()

    // Root scope changes must unmount the retained detail observer and release both live channels;
    // a keep-alive sidebar page may persist, but a child subscription may not cross roots.
    await rejectionScreen.getByRole('button', { name: 'select-conversation-other' }).click()
    await expect.poll(() => scenarioState.observerEventListeners.size).toBe(0)
    expect(scenarioState.agentEventListeners.size).toBe(rootAgentListenerCount)
    expect(
      rejectionScreen.container.querySelector('[data-conversation-surface-mode="observer"]')
    ).toBeNull()
  })

  it('converges through the durable sequence, survives remount and drops the old root scope', async () => {
    const first = await render(<AppShell />)
    await first.getByRole('button', { name: 'select-conversation-root' }).click()
    await expect.element(first.getByTestId('collaboration-timeline')).toBeVisible()

    scenarioState.currentTree = parseAgentTreeSnapshot(scenarioFixture.settledTree)
    emitCollaborationEvent(
      parseCollaborationEventEnvelope(scenarioFixture.settledEventPage.events.at(-1))
    )
    await first.getByRole('button', { name: 'Subagents' }).click()
    await expect
      .poll(() =>
        first.container
          .querySelector('.agent-center__row[data-agent-id="agent-review"]')
          ?.getAttribute('data-status')
      )
      .toBe('latest_completed')
    expect(first.container.querySelector('[data-semantic="completed"]')).toBeNull()
    expect(first.container.querySelector('[data-semantic="interrupted"]')).toBeNull()
    expect(
      first.container.querySelector(
        '.collaboration-timeline__activity[data-semantic="started"] [data-agent-id="agent-review"]'
      )
    ).not.toBeNull()
    await first.unmount()

    const reloaded = await render(<AppShell />)
    await reloaded.getByRole('button', { name: 'select-conversation-root' }).click()
    await expect.element(reloaded.getByTestId('collaboration-timeline')).toBeVisible()
    expect(
      reloaded.container.querySelector(
        '.collaboration-timeline__activity[data-semantic="started"] [data-agent-id="agent-compatibility"]'
      )
    ).not.toBeNull()
    expect(reloaded.container.querySelector('[data-semantic="completed"]')).toBeNull()
    expect(reloaded.container.querySelector('[data-semantic="interrupted"]')).toBeNull()
    expect(reloaded.container.querySelector('[data-semantic="waiting_approval"]')).toBeNull()
    await reloaded.getByRole('button', { name: 'Subagents' }).click()
    expect(
      requiredAgentCenterRow(reloaded.container, 'agent-compatibility').getAttribute('data-status')
    ).toBe('latest_interrupted')
    expect(
      requiredAgentCenterRow(reloaded.container, 'agent-review').getAttribute('data-status')
    ).toBe('latest_completed')
    expect(
      requiredAgentCenterRow(reloaded.container, 'agent-review').querySelector(
        '.agent-center__row-meta'
      )?.textContent
    ).toBe('Now')

    await reloaded.getByRole('button', { name: 'select-conversation-other' }).click()
    await expect
      .poll(() => reloaded.container.querySelector('[data-testid="collaboration-timeline"]'))
      .toBeNull()
    expect(
      reloaded.container.querySelector('[data-conversation-surface-mode="observer"]')
    ).toBeNull()
    expect(
      Array.from(reloaded.container.querySelectorAll('.right-sidebar__tool-title')).some(
        (element) => element.textContent === 'Subagents'
      )
    ).toBe(false)
  })
})
