import type { AgentDisplayStatusView, AgentSummary, AgentTreeSnapshot } from '@mycopilot/protocol'
import { PanelTop } from 'lucide-react'
import { page, userEvent } from 'vitest/browser'
import { afterAll, beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { CSSProperties, ReactNode } from 'react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import type { CollaborationStoreSnapshot } from '../../agentCollaboration/collaborationStore'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/collaborationTimelineModel'
import { CollaborationTimelineActivityList } from '../../agentCollaboration/CollaborationTimelineActivity'
import type {
  AgentObserverRenderContext,
  RightSidebarModuleDefinition,
  RightSidebarModuleNavigationRequest
} from '../rightSidebarTypes'

const TRANSLATIONS: Record<string, string> = {
  'collaboration.activity.copyAgentStatus': '{name}: {status}',
  'collaboration.activity.openAgentActivity': 'View sub-agent {name}: {status}',
  'collaboration.activity.status.started': 'Started working',
  'collaboration.activity.status.updated': 'Updated',
  'collaboration.activity.statusListSeparator': '; ',
  'agentCenter.user': 'User',
  'agentCenter.openRootAgent': 'Open parent agent {name}, base model {model}, status {status}',
  'settings.page.profile': 'Profile',
  'agentCenter.switchToOutline': 'Switch to directory tree',
  'agentCenter.switchToDiagram': 'Switch to diagram tree',
  'agentCenter.switchToList': 'Switch to list view',
  'agentCenter.treeView': 'Agent tree',
  'agentCenter.expandAgent': 'Expand children of {name}',
  'agentCenter.collapseAgent': 'Collapse children of {name}',
  'agentCenter.noAgents': 'No subagents yet.',
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
beforeEach(() => page.viewport(1000, 820))
afterEach(() => page.viewport(1280, 720))

const TEST_NOW = 1_700_000_060_000

const dateNowSpy = vi.spyOn(Date, 'now').mockReturnValue(TEST_NOW)
afterAll(() => vi.restoreAllMocks())

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) => TRANSLATIONS[key] ?? key
  })
}))

vi.mock('../../auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({
    state: {
      profile: { userId: 'test-user-1234567890', displayName: '测试用户名', avatarDataUrl: null }
    }
  })
}))

const { RightSidebar } = await import('../RightSidebar')
const openProfile = vi.fn()
const openRootConversation = vi.fn()
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
  it('keeps all three view buttons visible and selects each view directly', async () => {
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [agent('root-a', 'child-a', 'running')])}
      />
    )
    await screen.getByRole('button', { name: 'Subagents' }).click()
    const labels = ['Switch to list view', 'Switch to diagram tree', 'Switch to directory tree']
    const assertSelected = async (selected: string) => {
      for (const label of labels) {
        const button = screen.getByRole('button', { name: label, exact: true })
        await expect.element(button).toBeVisible()
        await expect.element(button).toHaveAttribute('aria-pressed', String(label === selected))
      }
      await expect
        .element(screen.getByRole('button', { name: 'Manage Agent templates' }))
        .toBeVisible()
      const center = requiredElement(screen.container, '.agent-center')
      expect(center.scrollWidth).toBeLessThanOrEqual(center.clientWidth)
    }
    await assertSelected(labels[0])
    await screen.getByRole('button', { name: labels[2] }).click()
    await assertSelected(labels[2])
    const tree = requiredElement(screen.container, '.agent-tree--outline')
    tree.scrollTop = 20
    const scrollTop = tree.scrollTop
    await screen.getByRole('button', { name: labels[2] }).click()
    expect(tree.scrollTop).toBe(scrollTop)
    await screen.getByRole('button', { name: labels[0] }).click()
    await assertSelected(labels[0])
    await screen.getByRole('button', { name: labels[1] }).click()
    await assertSelected(labels[1])
    expect(screen.container.querySelector('.agent-tree--diagram')).not.toBeNull()
    await screen.getByRole('button', { name: labels[2] }).click()
    await assertSelected(labels[2])
    expect(screen.container.querySelector('.agent-tree--outline')).not.toBeNull()
  })

  it('explains the agent page actions on hover and updates the layout hint after switching', async () => {
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={snapshot('root-a', [agent('root-a', 'child-a', 'running')])}
      />
    )
    await screen.getByRole('button', { name: 'Subagents' }).click()
    const assertHint = async (label: string) => {
      const button = screen.getByRole('button', { name: label, exact: true })
      await expect.element(button).toBeVisible()
      await userEvent.hover(button.element())
      await expect.element(page.getByRole('tooltip')).toHaveTextContent(label)
      expect(button.element().hasAttribute('title')).toBe(false)
      await userEvent.unhover(button.element())
    }
    await assertHint('Switch to diagram tree')
    await assertHint('Manage Agent templates')
    await screen.getByRole('button', { name: 'Switch to diagram tree' }).click()
    await assertHint('Switch to directory tree')
    await assertHint('Switch to list view')
    await screen.getByRole('button', { name: 'Switch to directory tree' }).click()
    await assertHint('Switch to diagram tree')
    await screen
      .getByRole('button', {
        name: 'Open subagent child-a, base model model-child-a, status Working'
      })
      .click()
    await assertHint('Back to subagents')
  })

  it('scopes observer activity to its owned tasks including cross-level dispatches and opens their agents', async () => {
    const parent = agent('root-a', 'parent', 'running')
    const grandchild = { ...agent('root-a', 'grandchild', 'running'), parentAgentId: 'parent' }
    const sibling = agent('root-a', 'sibling', 'running')
    const unrelated = {
      ...agent('root-a', 'great-grandchild', 'running'),
      parentAgentId: 'grandchild'
    }
    const activity = (agentId: string, sequence: number): CollaborationTimelineActivity => ({
      activityId: `event-${sequence}`,
      agentId,
      occurredAt: sequence,
      ownerAgentId:
        agentId === 'grandchild' ? 'parent' : agentId === 'sibling' ? 'root:root-a' : 'grandchild',
      ownerConversationId:
        agentId === 'grandchild'
          ? parent.conversationId
          : agentId === 'great-grandchild'
            ? grandchild.conversationId
            : 'another-conversation',
      anchorMessageId: null,
      traceBoundarySequence: null,
      runId: null,
      semantic: 'started',
      sequence,
      taskNameSnapshot: agentId,
      turnId: null,
      taskMessageId: 'task-' + agentId
    })
    const tree = {
      ...snapshot('root-a', [parent, grandchild, sibling, unrelated]),
      activities: [
        activity('grandchild', 1),
        activity('sibling', 2),
        activity('great-grandchild', 3),
        {
          ...activity('great-grandchild', 4),
          ownerAgentId: parent.agentId,
          ownerConversationId: parent.conversationId,
          taskMessageId: 'cross-level-task'
        },
        {
          ...activity('agent-root-a', 5),
          ownerAgentId: parent.agentId,
          ownerConversationId: parent.conversationId,
          taskMessageId: null,
          semantic: 'updated' as const
        }
      ]
    }
    const screen = await render(
      <NarrowSidebar
        activeConversationId="root-a"
        snapshot={tree}
        observerProbe={({ agent, activities, onOpenAgent }) => (
          <div data-testid={`agent-observer-${agent.agentId}`}>
            <CollaborationTimelineActivityList activities={activities} onOpenAgent={onOpenAgent} />
          </div>
        )}
      />
    )

    await screen.getByRole('button', { name: 'Subagents' }).click()
    await screen
      .getByRole('button', {
        name: 'Open subagent parent, base model model-parent, status Working'
      })
      .click()
    await expect.element(screen.getByTestId('agent-observer-parent')).toBeVisible()
    expect(
      screen.getByTestId('agent-observer-parent').element().querySelectorAll('[data-agent-id]')
    ).toHaveLength(3)
    await screen.getByRole('button', { name: 'View sub-agent agent-root-a: Updated' }).click()
    expect(openRootConversation).toHaveBeenCalledWith('root-a')
    await expect.element(screen.getByTestId('agent-observer-parent')).toBeVisible()
    await screen.getByRole('button', { name: 'View sub-agent grandchild: Started working' }).click()
    await expect.element(screen.getByTestId('agent-observer-grandchild')).toBeVisible()
    expect(
      screen
        .getByTestId('agent-observer-grandchild')
        .element()
        .querySelector('[data-agent-id]')
        ?.getAttribute('data-agent-id')
    ).toBe('great-grandchild')
  })

  for (const theme of ['light', 'dark'] as const) {
    it(`switches tree layouts without losing disclosure or navigation in ${theme} mode`, async () => {
      const parent = agent('root-a', 'parent', 'running', '父级智能体长中文名称')
      const child = { ...agent('root-a', 'child', 'idle'), parentAgentId: 'parent' }
      const sibling = agent('root-a', 'sibling', 'waiting_approval')
      const tree = snapshot('root-a', [parent, child, sibling])
      const screen = await render(
        <NarrowSidebar activeConversationId="root-a" snapshot={tree} theme={theme} height={540} />
      )
      await screen.getByRole('button', { name: 'Subagents' }).click()
      await expect
        .element(screen.getByRole('button', { name: 'Switch to diagram tree' }))
        .toBeVisible()
      await expect
        .element(screen.getByRole('button', { name: 'Switch to directory tree' }))
        .toBeVisible()
      await screen.getByRole('button', { name: 'Switch to diagram tree' }).click()
      await screen
        .getByRole('button', { name: 'Collapse children of 父级智能体长中文名称' })
        .click()
      await screen.getByRole('button', { name: 'Switch to directory tree' }).click()
      const viewport = requiredElement(screen.container, '.agent-tree--outline')
      expect(screen.container.querySelector('.agent-tree__node[data-agent-id="child"]')).toBeNull()
      await screen.getByRole('button', { name: 'Expand children of 父级智能体长中文名称' }).click()
      const nodes = Array.from(viewport.querySelectorAll<HTMLElement>('.agent-tree__node'))
      expect(nodes).toHaveLength(5)
      const rects = nodes.map((node) => node.getBoundingClientRect())
      for (let index = 1; index < rects.length; index += 1) {
        expect(rects[index].top).toBeGreaterThanOrEqual(rects[index - 1].bottom)
      }
      expect(rects[1].left).toBeGreaterThan(rects[0].left)
      expect(rects[2].left).toBeGreaterThan(rects[1].left)
      expect(rects[3].left).toBeGreaterThan(rects[2].left)
      expect(rects[4].left).toBe(rects[2].left)
      const disclosure = requiredElement(nodes[2], '.agent-tree__toggle').getBoundingClientRect()
      const avatar = requiredElement(nodes[2], '.agent-tree__avatar-link').getBoundingClientRect()
      expect(disclosure.right).toBeLessThanOrEqual(avatar.left)
      expect(getComputedStyle(nodes[0]).animationName).toBe('agent-tree-breathe')
      expect(getComputedStyle(nodes[3]).animationName).toBe('none')
      const center = requiredElement(screen.container, '.agent-center')
      expect(center.scrollWidth).toBeLessThanOrEqual(center.clientWidth)
      await page.screenshot({
        element: center,
        path: `__screenshots__/AgentCenterRightSidebar.browser.test.tsx/agent-tree-outline-${theme}-280.png`
      })
      await screen
        .getByRole('button', { name: 'Open subagent child, base model model-child, status Ready' })
        .click()
      await expect.element(screen.getByTestId('agent-observer-child')).toBeVisible()
      await screen.getByRole('button', { name: 'Back to subagents' }).click()
      await expect.poll(() => document.activeElement?.getAttribute('data-agent-id')).toBe('child')
      expect(screen.container.querySelector('.agent-tree--outline')).not.toBeNull()
      await screen.getByRole('button', { name: 'Collapse children of 测试用户名' }).click()
      await screen.getByRole('button', { name: 'Switch to diagram tree' }).click()
      expect(screen.container.querySelectorAll('.agent-tree__node')).toHaveLength(1)
      await screen.getByRole('button', { name: 'Expand children of 测试用户名' }).click()
      expect(screen.container.querySelectorAll('.agent-tree__node')).toHaveLength(5)
      await screen.getByRole('button', { name: 'Switch to directory tree' }).click()
      await screen.rerender(
        <NarrowSidebar
          activeConversationId="root-a"
          snapshot={tree}
          theme={theme}
          height={540}
          width={720}
        />
      )
      const wideNodes = Array.from(
        screen.container.querySelectorAll<HTMLElement>('.agent-tree__node')
      )
      expect(
        wideNodes[0].getBoundingClientRect().left -
          requiredElement(screen.container, '.agent-tree').getBoundingClientRect().left
      ).toBeLessThan(16)
      await page.screenshot({
        element: requiredElement(screen.container, '.agent-center'),
        path: `__screenshots__/AgentCenterRightSidebar.browser.test.tsx/agent-tree-outline-${theme}-720.png`
      })
      await screen.getByRole('button', { name: 'Switch to list view' }).click()
      await expect
        .element(screen.getByRole('button', { name: 'Switch to diagram tree' }))
        .toBeVisible()
      await screen.getByRole('button', { name: 'Switch to directory tree' }).click()
      expect(screen.container.querySelector('.agent-tree--outline')).not.toBeNull()
    })

    it(`shows a compact nested tree, restores navigation, and preserves the list in ${theme} mode`, async () => {
      const parent = agent(
        'root-a',
        'parent',
        'running',
        '父级智能体很长的中文名称用于测试紧凑布局'
      )
      const child = { ...agent('root-a', 'child', 'waiting_approval'), parentAgentId: 'parent' }
      const grandchild = { ...agent('root-a', 'grandchild', 'idle'), parentAgentId: 'child' }
      const sibling = { ...agent('root-a', 'sibling', 'latest_completed'), parentAgentId: 'parent' }
      const secondRoot = agent('root-a', 'second-root', 'queued')
      const tree = snapshot('root-a', [parent, child, grandchild, sibling, secondRoot])
      const screen = await render(
        <NarrowSidebar activeConversationId="root-a" snapshot={tree} theme={theme} height={360} />
      )
      await screen.getByRole('button', { name: 'Subagents' }).click()
      await expect
        .element(screen.getByRole('button', { name: 'Switch to diagram tree' }))
        .toBeVisible()
      const listRow = requiredElement(screen.container, '.agent-center__row')
      const before = {
        height: listRow.getBoundingClientRect().height,
        fontSize: getComputedStyle(requiredElement(listRow, 'strong')).fontSize,
        text: listRow.textContent
      }
      const actions = requiredElement(screen.container, '.agent-center__view-actions')
      expect(actions.querySelectorAll('button')[1].getAttribute('aria-label')).toBe(
        'Switch to diagram tree'
      )
      expect(actions.querySelectorAll('button')[3].getAttribute('aria-label')).toBe(
        'Manage Agent templates'
      )
      await screen.getByRole('button', { name: 'Switch to diagram tree' }).click()

      const treeViewport = requiredElement(screen.container, '.agent-tree')
      const parentBranch = requiredElement(
        screen.container,
        '.agent-tree__node[data-agent-id="parent"]'
      ).parentElement!
      expect(
        parentBranch.querySelectorAll(':scope > .agent-tree__children .agent-tree__node')
      ).toHaveLength(3)
      expect(screen.container.querySelectorAll('.agent-tree__roots > li')).toHaveLength(1)
      const rootNode = requiredElement(
        screen.container,
        '.agent-tree__node[data-agent-id="agent-root-a"]'
      )
      expect(rootNode.querySelector('.agent-tree__avatar--root svg')).not.toBeNull()
      expect(
        rootNode.parentElement?.querySelectorAll(':scope > .agent-tree__children > li')
      ).toHaveLength(2)
      expect(requiredElement(screen.container, '.agent-tree__node--user strong').textContent).toBe(
        '测试用户名'
      )
      expect(
        requiredElement(screen.container, '.agent-tree__node--user .agent-tree__copy > span')
          .textContent
      ).toBe('Captain Who')
      expect(
        getComputedStyle(requiredElement(screen.container, '.agent-tree__node--user')).animationName
      ).toBe('agent-tree-breathe')
      openProfile.mockClear()
      openRootConversation.mockClear()
      await screen.getByRole('button', { name: 'Profile', exact: true }).click()
      expect(openProfile).toHaveBeenCalledTimes(1)
      await screen
        .getByRole('button', {
          name: 'Open parent agent root, base model Root Model, status Ready'
        })
        .click()
      expect(openRootConversation).toHaveBeenCalledWith('root-a')
      await screen.getByRole('button', { name: 'Collapse children of 测试用户名' }).click()
      expect(screen.container.querySelector('.agent-tree__node[data-agent-id]')).toBeNull()
      await screen.getByRole('button', { name: 'Expand children of 测试用户名' }).click()
      expect(
        screen.container.querySelectorAll('.agent-tree__node[data-active="true"]')
      ).toHaveLength(3)
      const parentNode = requiredElement(
        screen.container,
        '.agent-tree__node[data-agent-id="parent"]'
      )
      const idleNode = requiredElement(
        screen.container,
        '.agent-tree__node[data-agent-id="grandchild"]'
      )
      expect(getComputedStyle(parentNode).animationName).toBe('agent-tree-breathe')
      expect(getComputedStyle(idleNode).animationName).toBe('none')
      expect(getComputedStyle(idleNode).borderTopWidth).toBe('1px')
      expect(requiredElement(parentNode, 'strong').title).toBe(parent.taskName)
      expect(
        parseFloat(getComputedStyle(requiredElement(parentNode, 'strong')).fontSize)
      ).toBeLessThan(parseFloat(before.fontSize))
      expect(treeViewport.scrollWidth).toBeGreaterThan(treeViewport.clientWidth)
      expect(getComputedStyle(treeViewport).overflowX).toBe('auto')
      const center = requiredElement(screen.container, '.agent-center')
      expect(center.scrollWidth).toBeLessThanOrEqual(center.clientWidth)
      await page.screenshot({
        element: center,
        path: `__screenshots__/AgentCenterRightSidebar.browser.test.tsx/agent-tree-${theme}-280.png`
      })

      const childToggle = screen.getByRole('button', { name: 'Collapse children of child' })
      await childToggle.click()
      expect(
        screen.container.querySelector('.agent-tree__node[data-agent-id="grandchild"]')
      ).toBeNull()
      expect(screen.container.querySelector('.agent-center__observer')).toBeNull()
      await expect
        .element(screen.getByRole('button', { name: 'Expand children of child' }))
        .toHaveAttribute('aria-expanded', 'false')

      const avatar = screen.getByRole('button', {
        name: 'Open subagent sibling, base model model-sibling, status Completed'
      })
      treeViewport.scrollLeft = treeViewport.scrollWidth - treeViewport.clientWidth
      const savedScrollLeft = treeViewport.scrollLeft
      ;(avatar.element() as HTMLButtonElement).focus({ preventScroll: true })
      await userEvent.keyboard('{Enter}')
      await expect.element(screen.getByTestId('agent-observer-sibling')).toBeVisible()
      await screen.getByRole('button', { name: 'Back to subagents' }).click()
      await expect.poll(() => document.activeElement?.getAttribute('data-agent-id')).toBe('sibling')
      expect(requiredElement(screen.container, '.agent-tree').scrollLeft).toBe(savedScrollLeft)
      expect(
        screen.container.querySelector('.agent-tree__node[data-agent-id="grandchild"]')
      ).toBeNull()
      await screen.getByRole('button', { name: 'Expand children of child' }).click()
      expect(
        screen.container.querySelector('.agent-tree__node[data-agent-id="grandchild"]')
      ).not.toBeNull()

      // Status changes must update the same tree from the existing collaboration snapshot.
      await screen.rerender(
        <NarrowSidebar
          activeConversationId="root-a"
          snapshot={snapshot('root-a', [
            { ...parent, displayStatus: 'idle' },
            child,
            grandchild,
            sibling,
            secondRoot
          ])}
          theme={theme}
          height={360}
        />
      )
      await expect
        .poll(() =>
          screen.container
            .querySelector('.agent-tree__node[data-agent-id="parent"]')
            ?.getAttribute('data-active')
        )
        .toBe('false')
      await screen.rerender(
        <NarrowSidebar activeConversationId="root-a" snapshot={tree} theme={theme} height={360} />
      )
      await screen.rerender(
        <NarrowSidebar
          activeConversationId="root-a"
          snapshot={tree}
          theme={theme}
          height={500}
          width={720}
        />
      )
      const wideTree = requiredElement(screen.container, '.agent-tree')
      expect(wideTree.scrollWidth).toBeLessThanOrEqual(wideTree.clientWidth)
      await page.screenshot({
        element: requiredElement(screen.container, '.agent-center'),
        path: `__screenshots__/AgentCenterRightSidebar.browser.test.tsx/agent-tree-${theme}-720.png`
      })
      await screen.rerender(
        <NarrowSidebar activeConversationId="root-a" snapshot={tree} theme={theme} height={360} />
      )
      await screen.getByRole('button', { name: 'Switch to list view' }).click()
      const restoredRow = requiredElement(screen.container, '.agent-center__row')
      expect({
        height: restoredRow.getBoundingClientRect().height,
        fontSize: getComputedStyle(requiredElement(restoredRow, 'strong')).fontSize,
        text: restoredRow.textContent
      }).toEqual(before)
    })
  }

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
    const homeCard = screen.container.querySelector<HTMLElement>('.right-sidebar__tool-card')
    expect(homeCard).not.toBeNull()
    expect(getComputedStyle(homeCard!).minHeight).toBe('36px')
    expect(getComputedStyle(homeCard!).justifyContent).toBe('center')
    const home = requiredElement(screen.container, '.right-sidebar__home')
    expect(getComputedStyle(home).justifyContent).toBe('center')

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
    await userEvent.unhover(listRow)
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
    await userEvent.unhover(row)
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
  width = 280,
  moduleNavigation,
  navigation,
  observerProbe,
  snapshot,
  theme = 'light'
}: {
  activeConversationId: string
  height?: number
  width?: number
  moduleNavigation?: RightSidebarModuleNavigationRequest
  navigation?: { agentId: string; requestId: number; rootConversationId: string }
  observerProbe?: (context: AgentObserverRenderContext) => ReactNode
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
    width
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
        onOpenAgentTemplates={NOOP}
        onOpenProfile={openProfile}
        onOpenAgentRootConversation={openRootConversation}
        onToggleMaximized={NOOP}
        renderAgentObserver={
          observerProbe ??
          (({ agent }) => (
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
          ))
        }
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
