import type { AgentDisplayStatusView, AgentSummary, AgentTreeSnapshot } from '@mycopilot/protocol'
import { PanelTop } from 'lucide-react'
import { page, userEvent } from 'vitest/browser'
import { afterAll, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { CSSProperties } from 'react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import type { CollaborationStoreSnapshot } from '../../agentCollaboration/collaborationStore'
import type {
  RightSidebarModuleDefinition,
  RightSidebarModuleNavigationRequest
} from '../rightSidebarTypes'

const TRANSLATIONS: Record<string, string> = {
  'agentCenter.active': 'In progress',
  'agentCenter.back': 'Back to subagents',
  'agentCenter.ended': 'Finished',
  'agentCenter.manageTemplates': 'Manage Agent templates',
  'agentCenter.modelUnavailable': 'Model unavailable',
  'agentCenter.noActive': 'No subagents are currently running.',
  'agentCenter.noCompleted': 'No finished subagents yet.',
  'agentCenter.observerUnavailable': 'This read-only conversation is unavailable.',
  'agentCenter.openAgent': 'Open subagent {name}, base model {model}, status {status}',
  'agentCenter.showMore': 'Show {count} more',
  'agentCenter.status.archived': 'Archived',
  'agentCenter.status.completed': 'Completed',
  'agentCenter.status.disabled': 'Disabled',
  'agentCenter.status.failed': 'Failed',
  'agentCenter.status.idle': 'Ready',
  'agentCenter.status.interrupted': 'Interrupted',
  'agentCenter.status.outcomeUnknown': 'Outcome unknown',
  'agentCenter.status.queued': 'Queued',
  'agentCenter.status.running': 'Working',
  'agentCenter.status.waitingApproval': 'Waiting for approval',
  'agentCenter.timeNow': 'Now',
  'agentCenter.timeUnknown': 'Unknown activity',
  'rightSidebar.agentCenter': 'Subagents',
  'rightSidebar.newPanel': 'New panel',
  'rightSidebar.terminal': 'Terminal'
}
const TEST_NOW = 1_700_000_060_000

const dateNowSpy = vi.spyOn(Date, 'now').mockReturnValue(TEST_NOW)
afterAll(() => vi.restoreAllMocks())

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) => TRANSLATIONS[key] ?? key
  })
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
  it('reuses the current agent center and returns from detail to the root list on module navigation', async () => {
    const tree = snapshot('root-a', [agent('root-a', 'child-a', 'running')])
    const renderSidebar = (requestId?: number) => (
      <NarrowSidebar
        activeConversationId="root-a"
        moduleNavigation={
          requestId === undefined
            ? undefined
            : {
                conversationId: 'root-a',
                moduleId: 'agent-center',
                requestId,
                workspaceKey: 'project-a',
                workspacePath: '/workspace/a'
              }
        }
        navigation={{ agentId: 'child-a', requestId: 1, rootConversationId: 'root-a' }}
        snapshot={tree}
      />
    )
    const screen = await render(renderSidebar())
    await expect.element(screen.getByTestId('agent-observer-child-a')).toBeVisible()
    const originalPage = requiredElement(screen.container, '.right-sidebar__page')
    await screen.rerender(renderSidebar(1))
    await expect
      .element(
        screen.getByRole('button', {
          name: 'Open subagent child-a, base model model-child-a, status Working'
        })
      )
      .toBeVisible()
    expect(screen.container.querySelector('[data-testid="agent-observer-child-a"]')).toBeNull()
    expect(requiredElement(screen.container, '.right-sidebar__page')).toBe(originalPage)
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)

    await screen
      .getByRole('button', {
        name: 'Open subagent child-a, base model model-child-a, status Working'
      })
      .click()
    await expect.element(screen.getByTestId('agent-observer-child-a')).toBeVisible()
    await screen.rerender(renderSidebar(1))
    await expect.element(screen.getByTestId('agent-observer-child-a')).toBeVisible()
    await screen.rerender(renderSidebar(2))
    await expect
      .poll(() => screen.container.querySelector('[data-testid="agent-observer-child-a"]'))
      .toBeNull()
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
  })

  it('rejects a module request after its child catalog disappears without reopening it later', async () => {
    const moduleNavigation: RightSidebarModuleNavigationRequest = {
      conversationId: 'root-a',
      moduleId: 'agent-center',
      requestId: 1,
      workspaceKey: 'project-a',
      workspacePath: '/workspace/a'
    }
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        moduleNavigation={moduleNavigation}
        snapshot={snapshot('root-a', [])}
      />
    )
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(0)
    await screen.rerender(
      <NarrowSidebar
        activeConversationId="root-a"
        moduleNavigation={moduleNavigation}
        snapshot={snapshot('root-a', [agent('root-a', 'child-a', 'running')])}
      />
    )
    await expect.poll(() => homeModuleLabels(screen.container)).toContain('Subagents')
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(0)
  })

  it('preserves the legacy home when the active root has no child and appears dynamically later', async () => {
    const screen = await render(
      <NarrowSidebar activeConversationId="root-a" snapshot={snapshot('root-a', [])} />
    )

    expect(homeModuleLabels(screen.container)).not.toContain('Subagents')
    expect(screen.container.querySelector('.right-sidebar__module-badge')).toBeNull()

    await screen.rerender(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [agent('root-a', 'child-running', 'running')])}
      />
    )

    await expect.poll(() => homeModuleLabels(screen.container)).toContain('Subagents')
    await expect.element(screen.getByRole('button', { name: 'Subagents' })).toBeVisible()
    expect(screen.container.querySelector('.right-sidebar__module-badge')?.textContent).toBe('1')
  })

  it('uses the localized unavailable label instead of exposing a model configuration ID', async () => {
    const internalId = '0197f53a-24e8-7a61-b630-secret-agent-model'
    const unavailableAgent = {
      ...agent('root-a', 'child-unavailable', 'idle'),
      model: { displayName: '   ', modelConfigId: internalId }
    }
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [unavailableAgent])}
      />
    )

    await screen.getByRole('button', { name: 'Subagents' }).click()
    await expect.element(screen.getByText('Model unavailable', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain(internalId)
  })

  it('groups active and ended agents, opens observer detail, and fits the 280px floor', async () => {
    const longTask = 'visual-review-nju-images-with-a-long-readable-suffix'
    const childRunning = agent('root-a', 'child-running', 'running', longTask, 'kimi')
    const childDone = agent('root-a', 'child-done', 'latest_completed', 'completed-task')
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [childRunning, childDone])}
      />
    )

    await screen.getByRole('button', { name: 'Subagents' }).click()
    expect(screen.container.querySelector('.agent-center__header')).toBeNull()
    await expect
      .poll(
        () => screen.container.querySelector('[aria-label="In progress"] h3')?.textContent ?? ''
      )
      .toBe('In progress·1')
    expect(screen.container.querySelector('[aria-label="Finished"] h3')?.textContent).toBe(
      'Finished·1'
    )
    await expect.element(screen.getByText(longTask, { exact: true })).toBeVisible()
    await expect.element(screen.getByText('kimi', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('model-child-running')
    expect(screen.container.querySelector('.agent-center__row')?.getAttribute('style')).toBeNull()

    const listCenter = requiredElement(screen.container, '.agent-center')
    const listRow = requiredElement(screen.container, '.agent-center__row')
    const listMeta = requiredElement(listRow, '.agent-center__row-meta')
    const listAvatar = requiredElement(listRow, '.agent-avatar')
    const listAvatarIndex = listAvatar.dataset.agentAvatarIndex
    const listAvatarSource = requiredElement(listAvatar, 'img').getAttribute('src')
    expect(listRow.title).toBe(`${longTask} · kimi`)
    expect(requiredElement(listRow, '.agent-center__row-copy strong').textContent).toBe(longTask)
    expect(listMeta.children).toHaveLength(1)
    expect(listMeta.textContent).toBe('Now')
    expect(getComputedStyle(listRow).borderTopWidth).toBe('0px')
    expect(getComputedStyle(listRow).backgroundColor).toBe('rgba(0, 0, 0, 0)')
    await userEvent.hover(listRow)
    expect(getComputedStyle(listRow).backgroundColor).not.toBe('rgba(0, 0, 0, 0)')
    await userEvent.unhover(listRow)
    expect(getComputedStyle(listRow).backgroundColor).toBe('rgba(0, 0, 0, 0)')
    expect(listCenter.scrollWidth).toBeLessThanOrEqual(listCenter.clientWidth)
    await page.screenshot({
      element: requiredElement(screen.container, '.right-sidebar'),
      path: '__screenshots__/AgentCenterRightSidebar.browser.test.tsx/agent-center-list-280.png'
    })

    const runningRow = screen.getByRole('button', {
      name: `Open subagent ${longTask}, base model kimi, status Working`
    })
    await tabUntil(runningRow.element() as HTMLButtonElement)
    await userEvent.keyboard('{Enter}')
    await expect.element(screen.getByTestId('agent-observer-child-running')).toBeVisible()
    await expect.element(screen.getByText('kimi', { exact: true })).toBeVisible()
    const detailAvatar = requiredElement(screen.container, '.agent-center__avatar--detail')
    expect(detailAvatar.dataset.agentAvatarIndex).toBe(listAvatarIndex)
    expect(requiredElement(detailAvatar, 'img').getAttribute('src')).toBe(listAvatarSource)

    const backButton = screen.getByRole('button', { name: 'Back to subagents' })
    await tabUntil(backButton.element() as HTMLButtonElement)
    expect(document.activeElement).toBe(backButton.element())

    const center = requiredElement(screen.container, '.agent-center')
    const observer = requiredElement(screen.container, '.agent-center__observer')
    const messages = requiredElement(screen.container, '.chat-conversation-page__messages')
    const observerStyle = getComputedStyle(messages)
    expect(observerStyle.paddingLeft).toBe('14px')
    expect(observerStyle.paddingRight).toBe('14px')
    expect(observerStyle.gap).toBe('40px')
    expect(center.scrollWidth).toBeLessThanOrEqual(center.clientWidth)
    expect(observer.scrollWidth).toBeLessThanOrEqual(observer.clientWidth)
    expect(screen.container.querySelector('.chat-composer')).toBeNull()
    expect(screen.container.querySelector('[aria-label="chat.send"]')).toBeNull()
    expect(summarySnapshot(screen.container)).toMatchInlineSnapshot(`
      {
        "activeCount": "",
        "detailAgent": "child-running",
        "endedCount": "",
        "hasComposer": false,
        "width": 280,
      }
    `)
    await page.screenshot({
      element: center,
      path: '__screenshots__/AgentCenterRightSidebar.browser.test.tsx/agent-center-observer-280.png'
    })
  })

  it('reveals bounded groups and restores the selected row, focus, and scroll after detail', async () => {
    const active = Array.from({ length: 6 }, (_, index) =>
      agent('root-a', `active-${String(index).padStart(2, '0')}`, 'running')
    )
    const ended = Array.from({ length: 12 }, (_, index) =>
      agent(
        'root-a',
        `ended-${String(index).padStart(2, '0')}`,
        index % 2 === 0 ? 'latest_completed' : 'latest_interrupted'
      )
    )
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        height={360}
        snapshot={snapshot('root-a', [...active, ...ended])}
      />
    )

    await screen.getByRole('button', { name: 'Subagents' }).click()
    const activeGroup = requiredElement(screen.container, '[aria-label="In progress"]')
    const endedGroup = requiredElement(screen.container, '[aria-label="Finished"]')
    expect(activeGroup.querySelectorAll('.agent-center__row')).toHaveLength(4)
    expect(endedGroup.querySelectorAll('.agent-center__row')).toHaveLength(10)

    const activeMore = requiredElement(activeGroup, '.agent-center__show-more')
    const endedMore = requiredElement(endedGroup, '.agent-center__show-more')
    expect(activeMore.textContent).toBe('Show 2 more')
    expect(endedMore.textContent).toBe('Show 2 more')
    await userEvent.click(activeMore)
    await userEvent.click(endedMore)
    expect(activeGroup.querySelectorAll('.agent-center__row')).toHaveLength(6)
    expect(endedGroup.querySelectorAll('.agent-center__row')).toHaveLength(12)

    const center = requiredElement(screen.container, '.agent-center')
    center.scrollTop = center.scrollHeight
    const capturedScrollTop = center.scrollTop
    expect(capturedScrollTop).toBeGreaterThan(0)
    const target = screen.getByRole('button', {
      name: 'Open subagent ended-11, base model model-ended-11, status Interrupted'
    })
    ;(target.element() as HTMLButtonElement).focus()
    await userEvent.keyboard('{Enter}')
    await expect.element(screen.getByTestId('agent-observer-ended-11')).toBeVisible()
    await screen.getByRole('button', { name: 'Back to subagents' }).click()

    await expect.poll(() => document.activeElement?.getAttribute('data-agent-id')).toBe('ended-11')
    const restoredCenter = requiredElement(screen.container, '.agent-center')
    const restoredRow = requiredElement(
      screen.container,
      '.agent-center__row[data-agent-id="ended-11"]'
    )
    expect(restoredCenter.scrollTop).toBe(capturedScrollTop)
    expect(restoredRow.hasAttribute('data-selected')).toBe(false)
    expect(getComputedStyle(restoredRow).backgroundColor).toBe('rgba(0, 0, 0, 0)')
    expect(restoredRow.title).toBe('ended-11 · model-ended-11')
    expect(requiredElement(restoredRow, '.agent-center__row-copy strong').title).toBe('ended-11')
    expect(requiredElement(restoredRow, '.agent-center__row-copy > span').title).toBe(
      'model-ended-11'
    )
  })

  it('uses product theme tokens in dark mode without restoring card chrome', async () => {
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [agent('root-a', 'child-dark', 'running')])}
        theme="dark"
      />
    )

    await screen.getByRole('button', { name: 'Subagents' }).click()
    const center = requiredElement(screen.container, '.agent-center')
    const row = requiredElement(screen.container, '.agent-center__row')
    const avatar = requiredElement(row, '.agent-center__avatar')
    const activityTime = requiredElement(row, '.agent-center__row-meta time')
    const tokenProbe = document.createElement('span')
    tokenProbe.style.color = 'var(--mc-color-text-muted)'
    center.append(tokenProbe)

    expect(getComputedStyle(row).borderTopWidth).toBe('0px')
    expect(getComputedStyle(row).backgroundColor).toBe('rgba(0, 0, 0, 0)')
    expect(getComputedStyle(avatar).backgroundColor).not.toBe('rgba(0, 0, 0, 0)')
    expect(getComputedStyle(activityTime).color).toBe(getComputedStyle(tokenProbe).color)
    expect(center.scrollWidth).toBeLessThanOrEqual(center.clientWidth)
  })

  it('expands and restores an externally opened row that started beyond the visible cap', async () => {
    const ended = Array.from({ length: 12 }, (_, index) =>
      agent('root-a', `ended-${String(index).padStart(2, '0')}`, 'latest_completed')
    )
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        navigation={{ agentId: 'ended-11', requestId: 1, rootConversationId: 'root-a' }}
        snapshot={snapshot('root-a', ended)}
      />
    )

    await expect.element(screen.getByTestId('agent-observer-ended-11')).toBeVisible()
    await screen.getByRole('button', { name: 'Back to subagents' }).click()
    await expect.poll(() => document.activeElement?.getAttribute('data-agent-id')).toBe('ended-11')

    const endedGroup = requiredElement(screen.container, '[aria-label="Finished"]')
    expect(endedGroup.querySelectorAll('.agent-center__row')).toHaveLength(12)
    const restoredRow = requiredElement(endedGroup, '.agent-center__row[data-agent-id="ended-11"]')
    expect(restoredRow.hasAttribute('data-selected')).toBe(false)
    expect(getComputedStyle(restoredRow).backgroundColor).toBe('rgba(0, 0, 0, 0)')
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
    expect(homeModuleLabels(screen.container)).not.toContain('Subagents')

    await screen.rerender(
      <NarrowSidebar
        activeConversationId="root-b"
        snapshot={snapshot('root-b', [agent('root-b', 'child-b', 'idle')])}
      />
    )
    await expect.poll(() => homeModuleLabels(screen.container)).toContain('Subagents')
    await screen.getByRole('button', { name: 'Subagents' }).click()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'Open subagent child-b, base model model-child-b, status Ready'
        })
      )
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

    await screen.getByRole('button', { name: 'Subagents' }).click()
    await screen
      .getByRole('button', {
        name: 'Open subagent child-a, base model model-child-a, status Working'
      })
      .click()
    const observer = screen.getByTestId('agent-observer-child-a').element()
    const agentCenterPage = observer.closest<HTMLElement>('.right-sidebar__page')
    if (!agentCenterPage) throw new Error('Missing Agent Center page')

    await screen.getByRole('button', { name: 'New panel' }).click()
    await screen.getByRole('menuitem', { name: 'Terminal' }).click()
    await expect.element(screen.getByTestId('keep-alive-test-surface')).toBeVisible()
    expect(agentCenterPage.getAttribute('aria-hidden')).toBe('true')
    expect(screen.getByTestId('agent-observer-child-a').element()).toBe(observer)

    await screen.getByRole('tab', { name: 'Subagents' }).click()
    await expect.element(screen.getByTestId('agent-observer-child-a')).toBeVisible()
    expect(screen.getByTestId('agent-observer-child-a').element()).toBe(observer)
  })

  it('refreshes recent activity time every minute and clears the timer on unmount', async () => {
    let intervalTick: (() => void) | null = null
    const intervalHandle = 42 as unknown as ReturnType<typeof window.setInterval>
    const setIntervalSpy = vi.spyOn(window, 'setInterval').mockImplementation((handler) => {
      if (typeof handler !== 'function') throw new Error('Expected an interval callback')
      intervalTick = () => handler()
      return intervalHandle
    })
    const clearIntervalSpy = vi.spyOn(window, 'clearInterval').mockImplementation(() => undefined)
    const active = agent('root-a', 'child-timer', 'running')
    active.latestActivityAt = TEST_NOW - 60_000

    const screen = await render(
      <NarrowSidebar activeConversationId="root-a" snapshot={snapshot('root-a', [active])} />
    )
    await screen.getByRole('button', { name: 'Subagents' }).click()
    const activityTime = requiredElement(
      screen.container,
      '.agent-center__row[data-agent-id="child-timer"] .agent-center__row-meta time'
    )
    expect(activityTime.textContent).toBe('1 minute ago')
    expect(setIntervalSpy).toHaveBeenCalledWith(expect.any(Function), 60_000)

    dateNowSpy.mockReturnValue(TEST_NOW + 60_000)
    const tick = intervalTick as (() => void) | null
    if (!tick) throw new Error('Missing Agent Center activity timer')
    tick()
    await expect.poll(() => activityTime.textContent).toBe('2 minutes ago')

    await screen.unmount()
    expect(clearIntervalSpy).toHaveBeenCalledWith(intervalHandle)
    setIntervalSpy.mockRestore()
    clearIntervalSpy.mockRestore()
    dateNowSpy.mockReturnValue(TEST_NOW)
  })
})

function NarrowSidebar({
  activeConversationId,
  height = 720,
  moduleNavigation,
  navigation,
  snapshot,
  theme = 'light'
}: {
  activeConversationId: string
  height?: number
  moduleNavigation?: RightSidebarModuleNavigationRequest
  navigation?: { agentId: string; requestId: number; rootConversationId: string }
  snapshot: CollaborationStoreSnapshot
  theme?: 'dark' | 'light'
}) {
  const sidebarStyle = {
    ...getFrontendCssVariables(
      frontendConfig,
      theme === 'dark' ? classicDarkTheme : classicLightTheme
    ),
    '--titlebar-height': 'var(--mc-layout-titlebar-height)',
    background: 'var(--mc-color-surface-main-panel)',
    color: 'var(--mc-color-text-primary)',
    fontFamily: 'var(--mc-font-family)',
    height,
    width: 280
  } as CSSProperties
  return (
    <div style={sidebarStyle}>
      <RightSidebar
        activeConversationId={activeConversationId}
        agentNavigationRequest={navigation}
        collaborationSnapshot={snapshot}
        isMaximized={false}
        isOpen
        modules={[KEEP_ALIVE_TEST_MODULE]}
        moduleNavigationRequest={moduleNavigation}
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
    activities: [],
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
  taskName = agentId,
  modelDisplayName = `model-${agentId}`
): AgentSummary {
  return {
    agentId,
    conversationId: `conversation-${agentId}`,
    displayStatus,
    latestActivityAt: 1_700_000_000_000 + agentId.length,
    lifecycle: 'active',
    model: { displayName: modelDisplayName, modelConfigId: `model-${agentId}` },
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
    activeCount: container.querySelector('[aria-label="In progress"] h3')?.textContent ?? '',
    endedCount: container.querySelector('[aria-label="Finished"] h3')?.textContent ?? '',
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
