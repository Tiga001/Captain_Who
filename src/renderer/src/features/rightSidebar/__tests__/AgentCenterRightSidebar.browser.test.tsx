import type { AgentDisplayStatusView, AgentSummary, AgentTreeSnapshot } from '@mycopilot/protocol'
import { PanelTop } from 'lucide-react'
import { page, userEvent } from 'vitest/browser'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { CollaborationStoreSnapshot } from '../../agentCollaboration/collaborationStore'
import type { RightSidebarModuleDefinition } from '../rightSidebarTypes'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

const { RightSidebar } = await import('../RightSidebar')
const NOOP = () => undefined
const KEEP_ALIVE_TEST_MODULE: RightSidebarModuleDefinition = {
  contextBinding: 'global',
  createPage: ({ pageId }) => ({
    id: pageId,
    moduleId: 'terminal',
    title: 'rightSidebar.terminal',
    workspaceKey: null
  }),
  icon: PanelTop,
  id: 'terminal',
  instancePolicy: 'single',
  render: () => <div data-testid="keep-alive-test-surface">terminal</div>,
  retention: 'keep-alive',
  surfaceKind: 'react',
  titleKey: 'rightSidebar.terminal',
  unavailablePagePolicy: 'retain-page'
}

describe('Agent Center right sidebar', () => {
  it('preserves the legacy home when the active root has no child and appears dynamically later', async () => {
    const screen = await render(
      <NarrowSidebar activeConversationId="root-a" snapshot={snapshot('root-a', [])} />
    )

    expect(homeModuleLabels(screen.container)).not.toContain('rightSidebar.agentCenter')
    expect(screen.container.querySelector('.right-sidebar__module-badge')).toBeNull()

    await screen.rerender(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [agent('root-a', 'child-running', 'running')])}
      />
    )

    await expect
      .poll(() => homeModuleLabels(screen.container))
      .toContain('rightSidebar.agentCenter')
    await expect
      .element(screen.getByRole('button', { name: 'rightSidebar.agentCenter' }))
      .toBeVisible()
    expect(screen.container.querySelector('.right-sidebar__module-badge')?.textContent).toBe('1')
  })

  it('groups active and completed agents, opens observer detail, and fits the 280px floor', async () => {
    const longTask = 'research-a-very-long-task-name-without-breaking-the-current-sidebar-layout'
    const childRunning = agent('root-a', 'child-running', 'running', longTask)
    const childDone = agent('root-a', 'child-done', 'latest_completed', 'completed-task')
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [childRunning, childDone])}
      />
    )

    await screen.getByRole('button', { name: 'rightSidebar.agentCenter' }).click()
    await expect.element(screen.getByRole('heading', { name: 'agentCenter.title' })).toBeVisible()
    expect(
      screen.container.querySelector('[aria-label="agentCenter.active"] h3')?.textContent
    ).toBe('agentCenter.active1')
    expect(screen.container.querySelector('.agent-center__completed summary')?.textContent).toBe(
      'agentCenter.completed1'
    )

    const runningRow = screen.getByRole('button', { name: `agentCenter.open ${longTask}` })
    await tabUntil(runningRow.element() as HTMLButtonElement)
    await userEvent.keyboard('{Enter}')
    await expect.element(screen.getByTestId('agent-observer-child-running')).toBeVisible()

    const backButton = screen.getByRole('button', { name: 'agentCenter.back' })
    await tabUntil(backButton.element() as HTMLButtonElement)
    expect(document.activeElement).toBe(backButton.element())

    const center = requiredElement(screen.container, '.agent-center')
    const observer = requiredElement(screen.container, '.agent-center__observer')
    const messages = requiredElement(screen.container, '.chat-conversation-page__messages')
    const observerStyle = getComputedStyle(messages)
    expect(observerStyle.paddingLeft).toBe('14px')
    expect(observerStyle.paddingRight).toBe('14px')
    expect(observerStyle.gap).toBe('28px')
    expect(center.scrollWidth).toBeLessThanOrEqual(center.clientWidth)
    expect(observer.scrollWidth).toBeLessThanOrEqual(observer.clientWidth)
    expect(screen.container.querySelector('.chat-composer')).toBeNull()
    expect(screen.container.querySelector('[aria-label="chat.send"]')).toBeNull()
    expect(summarySnapshot(screen.container)).toMatchInlineSnapshot(`
      {
        "activeCount": "",
        "completedCount": "",
        "detailAgent": "child-running",
        "hasComposer": false,
        "width": 280,
      }
    `)
    await page.screenshot({
      element: center,
      path: '__screenshots__/AgentCenterRightSidebar.browser.test.tsx/agent-center-observer-280.png'
    })
  })

  it('fails closed for an old snapshot and resets detail when another root in the project becomes active', async () => {
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        navigation={{ agentId: 'child-a', requestId: 1, rootConversationId: 'root-a' }}
        snapshot={snapshot('root-a', [agent('root-a', 'child-a', 'running')])}
      />
    )

    await expect.element(screen.getByTestId('agent-observer-child-a')).toBeVisible()

    // During the B-root hydration window the old A snapshot must not keep a visible Agent page.
    await screen.rerender(
      <NarrowSidebar
        activeConversationId="root-b"
        snapshot={snapshot('root-a', [agent('root-a', 'child-a', 'running')])}
      />
    )
    await expect
      .poll(() => screen.container.querySelector('[data-testid="agent-observer-child-a"]'))
      .toBeNull()
    expect(homeModuleLabels(screen.container)).not.toContain('rightSidebar.agentCenter')

    await screen.rerender(
      <NarrowSidebar
        activeConversationId="root-b"
        snapshot={snapshot('root-b', [agent('root-b', 'child-b', 'idle')])}
      />
    )
    await expect
      .poll(() => homeModuleLabels(screen.container))
      .toContain('rightSidebar.agentCenter')
    await screen.getByRole('button', { name: 'rightSidebar.agentCenter' }).click()
    await expect
      .element(screen.getByRole('button', { name: 'agentCenter.open child-b' }))
      .toBeVisible()
    expect(screen.container.querySelector('[data-testid="agent-observer-child-a"]')).toBeNull()
  })

  it('keeps the exact observer DOM mounted while another retained sidebar page is foregrounded', async () => {
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [agent('root-a', 'child-a', 'running')])}
      />
    )

    await screen.getByRole('button', { name: 'rightSidebar.agentCenter' }).click()
    await screen.getByRole('button', { name: 'agentCenter.open child-a' }).click()
    const observer = screen.getByTestId('agent-observer-child-a').element()
    const agentCenterPage = observer.closest<HTMLElement>('.right-sidebar__page')
    if (!agentCenterPage) throw new Error('Missing Agent Center page')

    await screen.getByRole('button', { name: 'rightSidebar.newPanel' }).click()
    await screen.getByRole('menuitem', { name: 'rightSidebar.terminal' }).click()
    await expect.element(screen.getByTestId('keep-alive-test-surface')).toBeVisible()
    expect(agentCenterPage.getAttribute('aria-hidden')).toBe('true')
    expect(screen.getByTestId('agent-observer-child-a').element()).toBe(observer)

    await screen.getByRole('tab', { name: 'rightSidebar.agentCenter' }).click()
    await expect.element(screen.getByTestId('agent-observer-child-a')).toBeVisible()
    expect(screen.getByTestId('agent-observer-child-a').element()).toBe(observer)
  })
})

function NarrowSidebar({
  activeConversationId,
  navigation,
  snapshot
}: {
  activeConversationId: string
  navigation?: { agentId: string; requestId: number; rootConversationId: string }
  snapshot: CollaborationStoreSnapshot
}) {
  return (
    <div style={{ height: 720, width: 280 }}>
      <RightSidebar
        activeConversationId={activeConversationId}
        agentNavigationRequest={navigation}
        collaborationSnapshot={snapshot}
        isMaximized={false}
        isOpen
        modules={[KEEP_ALIVE_TEST_MODULE]}
        onToggleMaximized={NOOP}
        renderAgentObserver={({ agent }) => (
          <div className="chat-conversation-page" data-testid={`agent-observer-${agent.agentId}`}>
            <div className="chat-conversation-page__messages-region">
              <div className="chat-conversation-page__messages">
                <article className="chat-message">
                  <div className="chat-message__content">
                    observer-content-that-must-not-force-horizontal-overflow-at-the-sidebar-floor
                  </div>
                </article>
              </div>
            </div>
          </div>
        )}
        workspaceKey="project-a"
        workspaceKeys={['project-a']}
        workspaceName="Project A"
        workspacePath="/workspace/a"
      />
    </div>
  )
}

function snapshot(
  rootConversationId: string,
  children: AgentSummary[]
): CollaborationStoreSnapshot {
  const tree: AgentTreeSnapshot = {
    agents: [rootAgent(rootConversationId), ...children],
    lastSequence: children.length + 1,
    projectId: 'project-a',
    rootAgentId: `agent-${rootConversationId}`,
    rootConversationId,
    schemaVersion: 1,
    workspaceId: 'workspace-a'
  }
  return {
    agentInvalidationSequences: Object.fromEntries(
      tree.agents.map((agent) => [agent.agentId, tree.lastSequence])
    ),
    error: false,
    hydrationRevision: 1,
    loading: false,
    rootConversationId,
    tree
  }
}

function rootAgent(rootConversationId: string): AgentSummary {
  return {
    agentId: `agent-${rootConversationId}`,
    conversationId: rootConversationId,
    displayStatus: 'idle',
    latestActivityAt: 1_700_000_000_000,
    lifecycle: 'active',
    model: { displayName: 'Root Model', modelConfigId: 'root-model' },
    parentAgentId: null,
    projectId: 'project-a',
    rootAgentId: `agent-${rootConversationId}`,
    rootConversationId,
    taskName: 'root',
    taskPath: '/root'
  }
}

function agent(
  rootConversationId: string,
  agentId: string,
  displayStatus: AgentDisplayStatusView,
  taskName = agentId
): AgentSummary {
  return {
    agentId,
    conversationId: `conversation-${agentId}`,
    displayStatus,
    latestActivityAt: 1_700_000_000_000 + agentId.length,
    lifecycle: 'active',
    model: { displayName: `${agentId} model`, modelConfigId: `model-${agentId}` },
    parentAgentId: `agent-${rootConversationId}`,
    projectId: 'project-a',
    rootAgentId: `agent-${rootConversationId}`,
    rootConversationId,
    taskName,
    taskPath: `/root/${agentId}`
  }
}

function homeModuleLabels(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll<HTMLElement>('.right-sidebar__tool-title')).map(
    (element) => element.textContent ?? ''
  )
}

function requiredElement(container: HTMLElement, selector: string): HTMLElement {
  const element = container.querySelector<HTMLElement>(selector)
  if (!element) throw new Error(`Missing ${selector}`)
  return element
}

function summarySnapshot(container: HTMLElement) {
  const shell = requiredElement(container, '.right-sidebar').parentElement
  return {
    activeCount: container.querySelector('[aria-label="agentCenter.active"] h3')?.textContent ?? '',
    completedCount: container.querySelector('.agent-center__completed summary')?.textContent ?? '',
    detailAgent: container.querySelector<HTMLElement>('.agent-center__observer')?.dataset.agentId,
    hasComposer: Boolean(container.querySelector('.chat-composer')),
    width: shell?.clientWidth ?? 0
  }
}

async function tabUntil(target: HTMLElement, limit = 8): Promise<void> {
  for (let attempt = 0; attempt < limit && document.activeElement !== target; attempt += 1) {
    await userEvent.keyboard('{Tab}')
  }
  expect(document.activeElement).toBe(target)
}
