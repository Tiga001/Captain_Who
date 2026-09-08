import { StrictMode, useEffect, useRef, useState } from 'react'
import { PanelTop } from 'lucide-react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  RightSidebarCapabilities,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarModuleNavigationRequest,
  RightSidebarModuleRenderProps
} from '../../rightSidebar/rightSidebarTypes'
import { createRightSidebarWorkspaceSessionKey } from '../../rightSidebar/rightSidebarWorkspace'
import { RightSidebar } from '../../rightSidebar/RightSidebar'
import { BottomPanel } from '../BottomPanel'
import '../../../styles/global.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const mounts = vi.fn()
const unmounts = vi.fn()
const noop = () => undefined
const workspaceA = {
  workspaceKey: 'project-a',
  workspacePath: '/repo/a',
  workspaceName: 'Project A'
}
const workspaceB = {
  workspaceKey: 'project-b',
  workspacePath: '/repo/b',
  workspaceName: 'Project B'
}
const modules: RightSidebarModuleDefinition[] = [
  createModule('terminal'),
  createModule('browser'),
  { ...createModule('files'), requiresWorkspace: true },
  {
    ...createModule('git-review'),
    contextBinding: 'follow-workspace',
    instancePolicy: 'single',
    requiredCapability: 'git-repository'
  }
]

function createModule(moduleId: RightSidebarModuleId): RightSidebarModuleDefinition {
  const titleKey = {
    terminal: 'rightSidebar.terminal',
    browser: 'rightSidebar.browser',
    files: 'rightSidebar.files',
    'git-review': 'rightSidebar.review',
    'agent-center': 'rightSidebar.agentCenter'
  } as const
  return {
    contextBinding: moduleId === 'browser' ? 'global' : 'pinned-to-creation-workspace',
    createPage: ({ existingPages, pageId, workspace }) => ({
      id: pageId,
      moduleId,
      title: `${moduleId} ${existingPages.filter((page) => page.moduleId === moduleId).length + 1}`,
      workspaceKey: workspace.key,
      workspacePath: workspace.path
    }),
    id: moduleId,
    icon: PanelTop,
    instancePolicy: 'multiple',
    render: (props) => <TrackedSurface {...props} />,
    retention: 'keep-alive',
    surfaceKind: 'react',
    titleKey: titleKey[moduleId],
    unavailablePagePolicy: 'retain-page'
  }
}

function TrackedSurface({ activity, page }: RightSidebarModuleRenderProps) {
  useEffect(() => {
    mounts(page.id)
    return () => {
      unmounts(page.id)
    }
  }, [page.id])
  return (
    <div
      data-testid={`${page.moduleId}-surface`}
      data-page-id={page.id}
      data-workspace={page.workspaceKey}
      data-activity={activity}
    >
      {page.title}
    </div>
  )
}

function capability(
  workspace = workspaceA,
  status: 'available' | 'checking' = 'available'
): RightSidebarCapabilities {
  return {
    'git-repository': {
      contextKey: createRightSidebarWorkspaceSessionKey(
        workspace.workspaceKey,
        workspace.workspacePath
      ),
      status
    }
  }
}

beforeEach(() => {
  mounts.mockClear()
  unmounts.mockClear()
})

describe('BottomPanel', () => {
  it('creates one new terminal per navigation request on first open, while open, and on reopen', async () => {
    const request = (requestId: number): RightSidebarModuleNavigationRequest => ({
      ...workspaceA,
      conversationId: 'conversation-a',
      moduleId: 'terminal',
      requestId
    })
    const fixture = (requestId: number, isOpen: boolean, isWorkspaceVisible = true) => (
      <StrictMode>
        <BottomPanel
          {...workspaceA}
          activeConversationId="conversation-a"
          isOpen={isOpen}
          isWorkspaceVisible={isWorkspaceVisible}
          moduleNavigationRequest={request(requestId)}
          modules={modules}
          onClose={noop}
          onOpenRightModule={noop}
        />
      </StrictMode>
    )
    const screen = await render(fixture(1, false))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(0)
    await screen.rerender(fixture(1, true, false))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(0)
    await screen.rerender(fixture(1, true))
    await expect.element(screen.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
    const first = screen.getByTestId('terminal-surface').element()
    await screen.rerender(fixture(1, true))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)

    await screen.rerender(fixture(2, true))
    await expect
      .element(screen.getByRole('tab', { name: 'terminal 2' }))
      .toHaveAttribute('aria-selected', 'true')
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(2)
    expect(first.isConnected).toBe(true)
    expect(first.getAttribute('data-activity')).toBe('background')
    await screen.rerender(fixture(2, false))
    await screen.rerender(fixture(2, true))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(2)

    await screen.rerender(fixture(3, false))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(2)
    await screen.rerender(fixture(3, true))
    await expect.element(screen.getByRole('tab', { name: 'terminal 3' })).toBeVisible()
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(3)
    await screen.rerender(fixture(2, true))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(3)
  })

  it('consumes stale workspace, conversation, and non-terminal requests without reopening them later', async () => {
    const request = (requestId: number): RightSidebarModuleNavigationRequest => ({
      ...workspaceA,
      conversationId: 'conversation-a',
      moduleId: 'terminal',
      requestId
    })
    const fixture = (
      navigation: RightSidebarModuleNavigationRequest | null,
      isOpen = true,
      workspace = workspaceA,
      conversationId = 'conversation-a'
    ) => (
      <BottomPanel
        {...workspace}
        activeConversationId={conversationId}
        isOpen={isOpen}
        isWorkspaceVisible
        moduleNavigationRequest={navigation}
        modules={modules}
        onClose={noop}
        onOpenRightModule={noop}
      />
    )
    const screen = await render(fixture(null))
    await expect.element(screen.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    const first = screen.getByTestId('terminal-surface').element()
    await screen.rerender(fixture(request(1), false))
    await screen.rerender(fixture(request(1), false, workspaceB))
    await screen.rerender(fixture(request(1)))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
    await screen.rerender(fixture(request(2), false))
    await screen.rerender(fixture(request(2), false, workspaceA, 'conversation-b'))
    await screen.rerender(fixture(request(2)))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
    await screen.rerender(fixture({ ...request(3), moduleId: 'browser' }))
    await screen.rerender(fixture(request(3)))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
    expect(screen.getByTestId('terminal-surface').element()).toBe(first)
    expect(unmounts).not.toHaveBeenCalled()
    await screen.rerender(fixture(request(4)))
    await expect.element(screen.getByRole('tab', { name: 'terminal 2' })).toBeVisible()
  })

  it('waits for availability before consuming the first navigation request', async () => {
    const navigation: RightSidebarModuleNavigationRequest = {
      ...workspaceA,
      conversationId: null,
      moduleId: 'terminal',
      requestId: 1
    }
    const terminalModules: RightSidebarModuleDefinition[] = [
      { ...createModule('terminal'), requiredCapability: 'git-repository' }
    ]
    const fixture = (status: 'checking' | 'available') => (
      <StrictMode>
        <BottomPanel
          {...workspaceA}
          capabilities={capability(workspaceA, status)}
          isOpen
          isWorkspaceVisible
          moduleNavigationRequest={navigation}
          modules={terminalModules}
          onClose={noop}
          onOpenRightModule={noop}
        />
      </StrictMode>
    )
    const screen = await render(fixture('checking'))
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(0)
    await screen.rerender(fixture('available'))
    await expect.element(screen.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
  })

  it('opens a terminal from a projectless new chat', async () => {
    const screen = await render(
      <BottomPanel
        isOpen
        isWorkspaceVisible
        moduleNavigationRequest={{
          conversationId: null,
          moduleId: 'terminal',
          requestId: 1,
          workspaceKey: null
        }}
        modules={modules}
        onClose={noop}
        onOpenRightModule={noop}
      />
    )
    await expect.element(screen.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
  })

  it('starts only when visible and keeps terminal pages pinned and mounted while hidden or switching workspaces', async () => {
    const fixture = (isOpen: boolean, isWorkspaceVisible: boolean, workspace = workspaceA) => (
      <BottomPanel
        {...workspace}
        isOpen={isOpen}
        isWorkspaceVisible={isWorkspaceVisible}
        modules={modules}
        onClose={noop}
        onOpenRightModule={noop}
      />
    )
    const screen = await render(fixture(false, true))
    expect(screen.container.querySelector('[data-testid="terminal-surface"]')).toBeNull()
    await screen.rerender(fixture(true, false))
    expect(screen.container.querySelector('[data-testid="terminal-surface"]')).toBeNull()
    await screen.rerender(fixture(true, true))
    await expect.element(screen.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    const first = screen.getByTestId('terminal-surface').element()
    const firstId = first.getAttribute('data-page-id')
    expect(mounts).toHaveBeenCalledTimes(1)

    await screen.rerender(fixture(false, true, workspaceB))
    expect(screen.getByTestId('terminal-surface').element()).toBe(first)
    expect(first.getAttribute('data-activity')).toBe('dormant')
    expect(first.getAttribute('data-workspace')).toBe('project-a')
    expect(unmounts).not.toHaveBeenCalled()
    await screen.rerender(fixture(true, true, workspaceB))
    expect(first.getAttribute('data-activity')).toBe('foreground')
    expect(mounts).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'rightSidebar.newPanel' }).click()
    await screen.getByRole('menuitem', { name: 'rightSidebar.terminal' }).click()
    await expect.element(screen.getByRole('tab', { name: 'terminal 2' })).toBeVisible()
    expect(first.getAttribute('data-activity')).toBe('background')
    const all = screen.container.querySelectorAll('[data-testid="terminal-surface"]')
    expect(all).toHaveLength(2)
    expect(all[1].getAttribute('data-workspace')).toBe('project-b')
    await screen.getByRole('tab', { name: 'terminal 1' }).click()
    expect(first.getAttribute('data-page-id')).toBe(firstId)
    expect(mounts).toHaveBeenCalledTimes(2)
    expect(unmounts).not.toHaveBeenCalled()
  })

  it('creates one initial tab under StrictMode, closes the final session, and starts fresh on reopen', async () => {
    const screen = await render(
      <StrictMode>
        <ClosableBottom />
      </StrictMode>
    )
    await expect.element(screen.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
    const id = screen.getByTestId('terminal-surface').element().getAttribute('data-page-id')
    await screen.getByRole('tab', { name: 'terminal 1' }).hover()
    await screen.getByRole('button', { name: 'rightSidebar.closeTab' }).click()
    await expect
      .element(screen.getByRole('button', { name: 'Reopen bottom' }))
      .toHaveTextContent('Reopen bottom')
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(0)
    expect(unmounts).toHaveBeenCalledWith(id)
    await screen.getByRole('button', { name: 'Reopen bottom' }).click()
    await expect.element(screen.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    expect(screen.getByTestId('terminal-surface').element().getAttribute('data-page-id')).not.toBe(
      id
    )
  })

  it('routes other modules to the right panel and preserves its singleton policy', async () => {
    const screen = await render(<PairedPanels />)
    const bottom = screen.getByRole('region', { name: 'app.bottomPanel' })
    await expect.element(bottom.getByRole('tab', { name: 'terminal 1' })).toBeVisible()
    const openReview = async () => {
      await bottom.getByRole('button', { name: 'rightSidebar.newPanel' }).click()
      await screen.getByRole('menuitem', { name: 'rightSidebar.review' }).click()
    }
    await openReview()
    await expect.element(screen.getByRole('tab', { name: 'git-review 1' })).toBeVisible()
    await openReview()
    expect(screen.container.querySelectorAll('[data-testid="git-review-surface"]')).toHaveLength(1)
    expect(bottom.element().querySelectorAll('[role="tab"]')).toHaveLength(1)
    expect(bottom.element().querySelector('[data-testid="git-review-surface"]')).toBeNull()
  })

  it('opens its menu upward within the viewport and returns focus on Escape', async () => {
    const screen = await render(
      <div style={{ position: 'fixed', bottom: 0, left: 0, width: 320, height: 180 }}>
        <BottomPanel
          {...workspaceA}
          capabilities={capability()}
          isOpen
          isWorkspaceVisible
          modules={modules}
          onClose={noop}
          onOpenRightModule={noop}
        />
      </div>
    )
    const button = screen.getByRole('button', { name: 'rightSidebar.newPanel' })
    await button.click()
    const menu = screen.getByRole('menu', { name: 'rightSidebar.tools' }).element()
    const rect = menu.getBoundingClientRect()
    const anchor = button.element().getBoundingClientRect()
    expect(rect.bottom).toBeLessThanOrEqual(anchor.top)
    expect(rect.top).toBeGreaterThanOrEqual(12)
    expect(rect.left).toBeGreaterThanOrEqual(12)
    expect(rect.right).toBeLessThanOrEqual(window.innerWidth - 12)
    document.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))
    await expect.poll(() => document.querySelector('[role="menu"]')).toBeNull()
    expect(document.activeElement).toBe(button.element())
  })
})

function ClosableBottom() {
  const [open, setOpen] = useState(true)
  return (
    <>
      <button onClick={() => setOpen(true)}>Reopen bottom</button>
      <BottomPanel
        {...workspaceA}
        isOpen={open}
        isWorkspaceVisible
        modules={modules}
        onClose={() => setOpen(false)}
        onOpenRightModule={noop}
      />
    </>
  )
}

function PairedPanels() {
  const requestId = useRef(0)
  const [request, setRequest] = useState<RightSidebarModuleNavigationRequest | null>(null)
  return (
    <>
      <div style={{ width: 400, height: 350 }}>
        <RightSidebar
          {...workspaceA}
          activeConversationId="conversation-a"
          capabilities={capability()}
          isMaximized={false}
          isOpen
          moduleNavigationRequest={request}
          modules={modules}
          onToggleMaximized={noop}
        />
      </div>
      <div style={{ width: 600, height: 250 }}>
        <BottomPanel
          {...workspaceA}
          activeConversationId="conversation-a"
          capabilities={capability()}
          isOpen
          isWorkspaceVisible
          modules={modules}
          onClose={noop}
          onOpenRightModule={(moduleId) =>
            setRequest({
              ...workspaceA,
              conversationId: 'conversation-a',
              moduleId,
              requestId: ++requestId.current
            })
          }
        />
      </div>
    </>
  )
}

describe('RightSidebar external module navigation', () => {
  it('waits for capability checks and discards requests whose workspace or conversation changed', async () => {
    const request = (requestId: number): RightSidebarModuleNavigationRequest => ({
      ...workspaceA,
      conversationId: 'conversation-a',
      moduleId: 'git-review',
      requestId
    })
    const fixture = (
      navigation: RightSidebarModuleNavigationRequest,
      status: 'checking' | 'available',
      workspace = workspaceA,
      conversationId = 'conversation-a'
    ) => (
      <RightSidebar
        {...workspace}
        activeConversationId={conversationId}
        capabilities={capability(workspace, status)}
        isMaximized={false}
        isOpen
        moduleNavigationRequest={navigation}
        modules={modules}
        onToggleMaximized={noop}
      />
    )
    const screen = await render(fixture(request(1), 'checking'))
    expect(screen.container.querySelector('[role="tab"]')).toBeNull()
    await screen.rerender(fixture(request(1), 'available'))
    await expect.element(screen.getByRole('tab', { name: 'git-review 1' })).toBeVisible()
    await screen.getByRole('tab', { name: 'git-review 1' }).hover()
    await screen.getByRole('button', { name: 'rightSidebar.closeTab' }).click()

    await screen.rerender(fixture(request(2), 'checking'))
    await screen.rerender(fixture(request(2), 'available', workspaceB))
    await screen.rerender(fixture(request(2), 'available'))
    expect(screen.container.querySelector('[role="tab"]')).toBeNull()
    await screen.rerender(fixture(request(3), 'checking'))
    await screen.rerender(fixture(request(3), 'available', workspaceA, 'conversation-b'))
    await screen.rerender(fixture(request(3), 'available'))
    expect(screen.container.querySelector('[role="tab"]')).toBeNull()
    await screen.rerender(fixture(request(4), 'available'))
    await expect.element(screen.getByRole('tab', { name: 'git-review 1' })).toBeVisible()
  })
})
