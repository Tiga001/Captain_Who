import { useEffect, useState } from 'react'
import { PanelTop } from 'lucide-react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  RightSidebarActivity,
  RightSidebarCapabilities,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarModuleNavigationRequest,
  RightSidebarModuleRenderProps
} from '../rightSidebarTypes'
import { createRightSidebarWorkspaceSessionKey } from '../rightSidebarWorkspace'
import { RIGHT_SIDEBAR_MODULES } from '../rightSidebarModules'

const { translate } = vi.hoisted(() => ({
  translate: (key: string) => key
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: translate
  })
}))

const { RightSidebar } = await import('../RightSidebar')
const surfaceLifecycleSpy = vi.fn()
const surfaceRenderSpy = vi.fn()
const NOOP = () => undefined
let documentVisibilityState: DocumentVisibilityState = 'visible'

const MODULES: RightSidebarModuleDefinition[] = [
  createTestModule('terminal', 'pinned-to-creation-workspace', 'multiple', 'retain-page'),
  createTestModule('browser', 'global', 'multiple', 'retain-page'),
  {
    ...createTestModule('files', 'pinned-to-creation-workspace', 'multiple', 'close-page'),
    orphanedWorkspacePolicy: 'close-page',
    requiresWorkspace: true,
    retention: 'unmount-when-inactive'
  },
  {
    ...createTestModule('git-review', 'follow-workspace', 'single', 'close-page'),
    requiredCapability: 'git-repository'
  }
]

beforeEach(() => {
  surfaceLifecycleSpy.mockClear()
  surfaceRenderSpy.mockClear()
  documentVisibilityState = 'visible'
  vi.spyOn(document, 'visibilityState', 'get').mockImplementation(() => documentVisibilityState)
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('RightSidebar workspace lifecycle', () => {
  it('opens one fresh browser page per navigation request and preserves existing page state', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const browserModule = RIGHT_SIDEBAR_MODULES.find((module) => module.id === 'browser')!
    const modules = MODULES.map((module) =>
      module.id === 'browser' ? { ...module, createPage: browserModule.createPage } : module
    )
    const renderSidebar = (requestId: number) => (
      <RightSidebar
        {...workspace}
        activeConversationId="conversation-a"
        isMaximized={false}
        isOpen
        modules={modules}
        moduleNavigationRequest={{
          ...workspace,
          conversationId: 'conversation-a',
          moduleId: 'browser',
          requestId
        }}
        onToggleMaximized={NOOP}
      />
    )
    const screen = await render(renderSidebar(1))
    await expect.poll(() => lifecycleCount('mount', 'browser')).toBe(1)
    const original = getSurface(screen.container, 'browser')
    const originalPageId = original.dataset.pageId
    const localState = original.querySelector('button')!
    localState.click()
    await expect.poll(() => localState.textContent).toBe('1')
    expect(original.dataset.browserUrl).toBe('https://example.com/existing')

    await screen.rerender(renderSidebar(1))
    expect(lifecycleCount('mount', 'browser')).toBe(1)

    await screen.rerender(renderSidebar(2))
    await expect.poll(() => lifecycleCount('mount', 'browser')).toBe(2)
    const surfaces = screen.container.querySelectorAll<HTMLElement>(
      '[data-testid="browser-surface"]'
    )
    expect(surfaces).toHaveLength(2)
    expect(surfaces[0]).toBe(original)
    expect(surfaces[0]?.dataset.activity).toBe('background')
    expect(surfaces[0]?.querySelector('button')?.textContent).toBe('1')
    expect(surfaces[0]?.dataset.browserUrl).toBe('https://example.com/existing')
    expect(surfaces[1]?.dataset.pageId).not.toBe(originalPageId)
    expect(surfaces[1]?.dataset.activity).toBe('foreground')
    expect(surfaces[1]?.querySelector('button')?.textContent).toBe('0')
    expect(surfaces[1]?.dataset.browserUrl).toBeUndefined()
    expect(getTabLabels(screen.container)).toEqual(['browser.newTab', 'browser.newTab (1)'])
    expect(lifecycleCount('unmount', 'browser')).toBe(0)

    await screen.rerender(renderSidebar(1))
    expect(lifecycleCount('mount', 'browser')).toBe(2)
  })

  it('reuses review navigation without resetting its selected target or local view', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const reviewRequest = {
      kind: 'git-review' as const,
      filePath: 'src/selected.ts',
      projectId: 'project-a',
      requestId: 1,
      target: { kind: 'lastTurn' as const, conversationId: 'conversation-a' }
    }
    const renderSidebar = (requestId?: number) => (
      <RightSidebar
        {...workspace}
        activeConversationId="conversation-a"
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        moduleNavigationRequest={
          requestId === undefined
            ? undefined
            : {
                ...workspace,
                conversationId: 'conversation-a',
                moduleId: 'git-review',
                requestId
              }
        }
        onToggleMaximized={NOOP}
        reviewNavigationRequest={reviewRequest}
      />
    )
    const screen = await render(renderSidebar())
    await expect.poll(() => lifecycleCount('mount', 'git-review')).toBe(1)
    const review = getSurface(screen.container, 'git-review')
    const localState = review.querySelector('button')!
    localState.click()
    await expect.poll(() => localState.textContent).toBe('1')
    await openAdditionalModule(screen, 'rightSidebar.browser')

    await screen.rerender(renderSidebar(1))
    await expect.poll(() => review.dataset.activity).toBe('foreground')
    expect(getSurface(screen.container, 'git-review')).toBe(review)
    expect(review.dataset.reviewScope).toBe('lastTurn')
    expect(review.dataset.reviewFilePath).toBe('src/selected.ts')
    expect(review.dataset.reviewRequestId).toBe('1')
    expect(localState.textContent).toBe('1')

    await screen.rerender(renderSidebar(2))
    expect(lifecycleCount('mount', 'git-review')).toBe(1)
    expect(localState.textContent).toBe('1')
  })

  it('waits for current repository availability and opens a pending request only once', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const renderSidebar = (status: 'checking' | 'available') => (
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, status)}
        isMaximized={false}
        isOpen
        modules={MODULES}
        moduleNavigationRequest={{ ...workspace, moduleId: 'git-review', requestId: 1 }}
        onToggleMaximized={NOOP}
      />
    )
    const screen = await render(renderSidebar('checking'))
    expect(getTabLabels(screen.container)).toEqual([])
    await screen.rerender(renderSidebar('available'))
    await expect.poll(() => lifecycleCount('mount', 'git-review')).toBe(1)
    clickTestButton(screen.container.querySelector('.right-sidebar__tab-close')!)
    await expect.poll(() => getTabLabels(screen.container)).toEqual([])
    await screen.rerender(renderSidebar('available'))
    expect(getTabLabels(screen.container)).toEqual([])
  })

  it.each(['workspace', 'conversation'] as const)(
    'discards a pending navigation after its %s changes and never replays it on return',
    async (changedContext) => {
      const workspaceA = workspaceProps('project-a', 'Project A', '/repo/a')
      const workspaceB = workspaceProps('project-b', 'Project B', '/repo/b')
      const request: RightSidebarModuleNavigationRequest = {
        ...workspaceA,
        conversationId: 'conversation-a',
        moduleId: 'git-review',
        requestId: 1
      }
      const renderSidebar = (changed: boolean, status: 'checking' | 'available') => {
        const workspace = changed && changedContext === 'workspace' ? workspaceB : workspaceA
        return (
          <RightSidebar
            {...workspace}
            activeConversationId={changed ? 'conversation-b' : 'conversation-a'}
            capabilities={gitCapability(workspace, status)}
            isMaximized={false}
            isOpen
            modules={MODULES}
            moduleNavigationRequest={request}
            onToggleMaximized={NOOP}
          />
        )
      }
      const screen = await render(renderSidebar(false, 'checking'))
      await screen.rerender(renderSidebar(true, 'available'))
      expect(getTabLabels(screen.container)).toEqual([])
      await screen.rerender(renderSidebar(false, 'available'))
      expect(getTabLabels(screen.container)).toEqual([])
      expect(lifecycleCount('mount', 'git-review')).toBe(0)
    }
  )

  it('discards navigation when a checking module is removed before it becomes available', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const renderSidebar = (
      modules: RightSidebarModuleDefinition[],
      status: 'checking' | 'available'
    ) => (
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, status)}
        isMaximized={false}
        isOpen
        modules={modules}
        moduleNavigationRequest={{ ...workspace, moduleId: 'git-review', requestId: 1 }}
        onToggleMaximized={NOOP}
      />
    )
    const screen = await render(renderSidebar(MODULES, 'checking'))
    await screen.rerender(
      renderSidebar(
        MODULES.filter((module) => module.id !== 'git-review'),
        'available'
      )
    )
    await screen.rerender(renderSidebar(MODULES, 'available'))
    expect(getTabLabels(screen.container)).toEqual([])
  })

  it('reloads review in place while preserving terminal and browser surfaces', async () => {
    const workspaceA = workspaceProps('project-a', 'Project A', '/repo/a')
    const workspaceB = workspaceProps('project-b', 'Project B', '/repo/b')
    const screen = await render(
      <RightSidebar
        {...workspaceA}
        capabilities={gitCapability(workspaceA, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )

    await openHomeModule(screen, 'rightSidebar.terminal')
    await openAdditionalModule(screen, 'rightSidebar.browser')
    await openAdditionalModule(screen, 'rightSidebar.review')

    await expect.poll(() => lifecycleCount('mount', 'terminal')).toBe(1)
    await expect.poll(() => lifecycleCount('mount', 'browser')).toBe(1)
    await expect.poll(() => lifecycleCount('mount', 'git-review')).toBe(1)
    expect(getTabLabels(screen.container)).toEqual([
      'rightSidebar.terminal',
      'rightSidebar.browser',
      'rightSidebar.review'
    ])
    const terminalRendersBeforeWorkspaceSwitch = renderCount('terminal')
    const browserRendersBeforeWorkspaceSwitch = renderCount('browser')

    await screen.rerender(
      <RightSidebar
        {...workspaceB}
        capabilities={gitCapability(workspaceB, 'checking')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )

    await expect
      .poll(() => screen.container.querySelector('[data-testid="review-loading"]'))
      .not.toBeNull()
    expect(screen.container.querySelector('[data-testid="git-review-surface"]')).toBeNull()
    expect(lifecycleCount('unmount', 'git-review')).toBe(1)
    expect(lifecycleCount('mount', 'terminal')).toBe(1)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('mount', 'browser')).toBe(1)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
    expect(renderCount('terminal')).toBe(terminalRendersBeforeWorkspaceSwitch)
    expect(renderCount('browser')).toBe(browserRendersBeforeWorkspaceSwitch)
    expect(getTabLabels(screen.container)).toEqual([
      'rightSidebar.terminal',
      'rightSidebar.browser',
      'rightSidebar.review'
    ])

    await screen.rerender(
      <RightSidebar
        {...workspaceB}
        capabilities={gitCapability(workspaceB, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )

    await expect
      .poll(() => screen.container.querySelector('[data-testid="git-review-surface"]'))
      .not.toBeNull()
    const reviewB = getSurface(screen.container, 'git-review')
    expect(reviewB.dataset.workspaceKey).toBe('project-b')
    await expect.poll(() => lifecycleCount('mount', 'git-review')).toBe(2)
    expect(getSurface(screen.container, 'terminal').dataset.workspaceKey).toBe('project-a')
    expect(lifecycleCount('mount', 'terminal')).toBe(1)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('mount', 'browser')).toBe(1)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
    expect(renderCount('terminal')).toBe(terminalRendersBeforeWorkspaceSwitch)
    expect(renderCount('browser')).toBe(browserRendersBeforeWorkspaceSwitch)

    await screen.rerender(
      <RightSidebar
        {...workspaceB}
        capabilities={gitCapability(workspaceB, 'unavailable')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )

    await expect
      .poll(() => getTabLabels(screen.container))
      .toEqual(['rightSidebar.terminal', 'rightSidebar.browser'])
    expect(screen.container.querySelector('[data-testid="git-review-surface"]')).toBeNull()
    expect(lifecycleCount('unmount', 'git-review')).toBe(2)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
  })

  it('does not remount review for another conversation in the same workspace', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const screen = await render(
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )
    await openHomeModule(screen, 'rightSidebar.review')
    await expect.poll(() => lifecycleCount('mount', 'git-review')).toBe(1)

    await screen.rerender(
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )

    expect(lifecycleCount('mount', 'git-review')).toBe(1)
    expect(lifecycleCount('unmount', 'git-review')).toBe(0)
  })

  it('opens and reactivates last-turn review without resetting its mounted page state', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const renderSidebar = (requestId?: number, filePath?: string) => (
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
        reviewNavigationRequest={
          requestId === undefined
            ? null
            : {
                kind: 'git-review',
                projectId: workspace.workspaceKey,
                requestId,
                target: { kind: 'lastTurn', conversationId: 'conversation-1' },
                ...(filePath ? { filePath } : {})
              }
        }
      />
    )
    const screen = await render(renderSidebar())

    await openHomeModule(screen, 'rightSidebar.terminal')
    await screen.rerender(renderSidebar(1))
    await expect
      .poll(() => getSurface(screen.container, 'git-review').dataset.activity)
      .toBe('foreground')
    expect(getSurface(screen.container, 'git-review').dataset.reviewScope).toBe('lastTurn')
    expect(getSurface(screen.container, 'git-review').dataset.reviewRequestId).toBe('1')
    const localStateButton = getSurface(screen.container, 'git-review').querySelector('button')
    if (!localStateButton) throw new Error('Missing review-local state control')
    localStateButton.click()
    await expect.poll(() => localStateButton.textContent).toBe('1')

    await screen.getByRole('tab', { name: 'rightSidebar.terminal' }).click()
    await expect
      .poll(() => getSurface(screen.container, 'git-review').dataset.activity)
      .toBe('background')

    await screen.rerender(renderSidebar(2, 'src/target.ts'))
    await expect
      .poll(() => getSurface(screen.container, 'git-review').dataset.activity)
      .toBe('foreground')
    expect(getSurface(screen.container, 'git-review').dataset.reviewRequestId).toBe('2')
    expect(getSurface(screen.container, 'git-review').dataset.reviewFilePath).toBe('src/target.ts')
    await expect.poll(() => localStateButton.textContent).toBe('1')
    expect(lifecycleCount('mount', 'git-review')).toBe(1)
    expect(lifecycleCount('unmount', 'git-review')).toBe(0)

    await screen.rerender(renderSidebar(3))
    await expect.poll(() => localStateButton.textContent).toBe('1')
    expect(lifecycleCount('mount', 'git-review')).toBe(1)
  })

  it('keeps files pinned across conversations without disturbing retained modules', async () => {
    const workspaceA = workspaceProps('project-a', 'Project A', '/repo/a')
    const workspaceB = workspaceProps('project-b', 'Project B', '/repo/b')
    const renderSidebar = (workspace: typeof workspaceA) => (
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
        workspaceKeys={['project-a', 'project-b']}
      />
    )
    const screen = await render(renderSidebar(workspaceA))

    await openHomeModule(screen, 'rightSidebar.terminal')
    await openAdditionalModule(screen, 'rightSidebar.browser')
    await openAdditionalModule(screen, 'rightSidebar.review')
    await openAdditionalModule(screen, 'rightSidebar.files')
    await expect.poll(() => lifecycleCount('mount', 'files')).toBe(1)
    const terminalMountCount = lifecycleCount('mount', 'terminal')
    const browserMountCount = lifecycleCount('mount', 'browser')

    await screen.rerender(renderSidebar(workspaceB))

    await expect
      .poll(() => getSurface(screen.container, 'files').dataset.workspaceKey)
      .toBe('project-a')
    expect(getSurface(screen.container, 'terminal').dataset.workspaceKey).toBe('project-a')
    expect(getSurface(screen.container, 'git-review').dataset.workspaceKey).toBe('project-b')
    expect(lifecycleCount('mount', 'files')).toBe(1)
    expect(lifecycleCount('unmount', 'files')).toBe(0)
    expect(lifecycleCount('mount', 'terminal')).toBe(terminalMountCount)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('mount', 'browser')).toBe(browserMountCount)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
  })

  it('renders transient file tabs in italics and stabilizes them on a repeated open', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const screen = await render(
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )

    await openHomeModule(screen, 'rightSidebar.files')
    clickTestButton(screen.getByTestId('files-open-readme').element())

    const transientTab = screen.getByRole('tab', { name: 'README.md' })
    await expect.element(transientTab).toHaveAttribute('data-file-preview-state', 'transient')
    expect(getComputedStyle(transientTab.element().querySelector('span')!).fontStyle).toBe('italic')

    clickTestButton(screen.getByTestId('files-open-readme').element())
    await expect.element(transientTab).not.toHaveAttribute('data-file-preview-state')
    expect(getComputedStyle(transientTab.element().querySelector('span')!).fontStyle).toBe('normal')

    clickTestButton(screen.getByTestId('files-open-index').element())
    await expect.poll(() => getTabLabels(screen.container)).toEqual(['README.md', 'index.ts'])
    await expect
      .element(screen.getByRole('tab', { name: 'README.md' }))
      .not.toHaveAttribute('data-file-preview-state')
    await expect
      .element(screen.getByRole('tab', { name: 'index.ts' }))
      .toHaveAttribute('data-file-preview-state', 'transient')
  })

  it('removes only file pages whose project was deleted', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const renderSidebar = (workspaceKeys: readonly string[]) => (
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
        workspaceKeys={workspaceKeys}
      />
    )
    const screen = await render(renderSidebar(['project-a']))

    await openHomeModule(screen, 'rightSidebar.terminal')
    await openAdditionalModule(screen, 'rightSidebar.browser')
    await openAdditionalModule(screen, 'rightSidebar.files')
    await expect.poll(() => lifecycleCount('mount', 'files')).toBe(1)

    await screen.rerender(renderSidebar([]))

    await expect
      .poll(() => getTabLabels(screen.container))
      .toEqual(['rightSidebar.terminal', 'rightSidebar.browser'])
    expect(screen.container.querySelector('[data-testid="files-surface"]')).toBeNull()
    expect(lifecycleCount('unmount', 'files')).toBe(1)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
  })

  it('unmounts only inactive file UI while preserving keep-alive pages', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const capabilities = gitCapability(workspace, 'available')
    const screen = await render(
      <RightSidebar
        {...workspace}
        capabilities={capabilities}
        isMaximized={false}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )

    await openHomeModule(screen, 'rightSidebar.terminal')
    await openAdditionalModule(screen, 'rightSidebar.browser')
    await openAdditionalModule(screen, 'rightSidebar.files')
    await expect
      .poll(() => getSurface(screen.container, 'files').dataset.activity)
      .toBe('foreground')

    expect(getSurface(screen.container, 'terminal').dataset.activity).toBe('background')
    expect(getSurface(screen.container, 'browser').dataset.activity).toBe('background')
    const terminalRenderCount = renderCount('terminal')
    const terminalMountCount = lifecycleCount('mount', 'terminal')

    await screen.getByRole('tab', { name: 'rightSidebar.browser' }).click()

    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('foreground')
    expect(screen.container.querySelector('[data-testid="files-surface"]')).toBeNull()
    expect(lifecycleCount('unmount', 'files')).toBe(1)
    expect(getSurface(screen.container, 'terminal').dataset.activity).toBe('background')
    expect(renderCount('terminal')).toBe(terminalRenderCount)
    expect(lifecycleCount('mount', 'terminal')).toBe(terminalMountCount)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)

    await screen.getByRole('tab', { name: 'rightSidebar.files' }).click()
    await expect.poll(() => lifecycleCount('mount', 'files')).toBe(2)
    expect(getSurface(screen.container, 'files').dataset.workspaceKey).toBe('project-a')
  })

  it('marks every retained page dormant when the sidebar or document is hidden', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const capabilities = gitCapability(workspace, 'available')
    const renderSidebar = (isOpen: boolean, isMaximized = false) => (
      <RightSidebar
        {...workspace}
        capabilities={capabilities}
        isMaximized={isMaximized}
        isOpen={isOpen}
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )
    const screen = await render(renderSidebar(true))

    await openHomeModule(screen, 'rightSidebar.terminal')
    await openAdditionalModule(screen, 'rightSidebar.browser')
    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('foreground')

    documentVisibilityState = 'hidden'
    document.dispatchEvent(new Event('visibilitychange'))
    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('dormant')
    expect(getSurface(screen.container, 'terminal').dataset.activity).toBe('dormant')
    expect(getComputedStyle(getPageFrame(screen.container, 'browser')).contentVisibility).toBe(
      'hidden'
    )

    documentVisibilityState = 'visible'
    document.dispatchEvent(new Event('visibilitychange'))
    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('foreground')
    expect(getComputedStyle(getPageFrame(screen.container, 'browser')).contentVisibility).toBe(
      'visible'
    )

    await screen.rerender(renderSidebar(false))
    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('dormant')
    expect(getSurface(screen.container, 'terminal').dataset.activity).toBe('dormant')

    await screen.rerender(renderSidebar(false, true))
    expect(getSurface(screen.container, 'browser').dataset.activity).toBe('dormant')
    expect(getSurface(screen.container, 'terminal').dataset.activity).toBe('dormant')

    await screen.rerender(renderSidebar(true))
    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('foreground')
    expect(getSurface(screen.container, 'terminal').dataset.activity).toBe('background')
    expect(lifecycleCount('mount', 'terminal')).toBe(1)
    expect(lifecycleCount('mount', 'browser')).toBe(1)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
  })

  it('does not render module surfaces for unrelated sidebar parent updates', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const capabilities = gitCapability(workspace, 'available')
    const renderSidebar = (isMaximized: boolean) => (
      <RightSidebar
        {...workspace}
        capabilities={capabilities}
        isMaximized={isMaximized}
        isOpen
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )
    const screen = await render(renderSidebar(false))

    await openHomeModule(screen, 'rightSidebar.terminal')
    await openAdditionalModule(screen, 'rightSidebar.browser')
    await openAdditionalModule(screen, 'rightSidebar.files')
    await expect.poll(() => lifecycleCount('mount', 'files')).toBe(1)
    const rendersBefore = new Map(
      (['terminal', 'browser', 'files'] as const).map((moduleId) => [
        moduleId,
        renderCount(moduleId)
      ])
    )

    await screen.rerender(renderSidebar(true))

    expect(renderCount('terminal')).toBe(rendersBefore.get('terminal'))
    expect(renderCount('browser')).toBe(rendersBefore.get('browser'))
    expect(renderCount('files')).toBe(rendersBefore.get('files'))
  })

  it('keeps module surfaces mounted while the workspace is covered by settings', async () => {
    const workspace = workspaceProps('project-a', 'Project A', '/repo/a')
    const renderSidebar = (isWorkspaceVisible: boolean) => (
      <RightSidebar
        {...workspace}
        isMaximized={false}
        isOpen
        isWorkspaceVisible={isWorkspaceVisible}
        modules={MODULES}
        onToggleMaximized={NOOP}
      />
    )
    const screen = await render(renderSidebar(true))

    await openHomeModule(screen, 'rightSidebar.terminal')
    await openAdditionalModule(screen, 'rightSidebar.browser')
    await expect.poll(() => lifecycleCount('mount', 'terminal')).toBe(1)
    await expect.poll(() => lifecycleCount('mount', 'browser')).toBe(1)

    await screen.rerender(renderSidebar(false))

    await expect
      .poll(() => getSurface(screen.container, 'terminal').dataset.activity)
      .toBe('dormant')
    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('dormant')
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
    expect(getTabLabels(screen.container)).toEqual([
      'rightSidebar.terminal',
      'rightSidebar.browser'
    ])

    await screen.rerender(renderSidebar(true))

    await expect
      .poll(() => getSurface(screen.container, 'terminal').dataset.activity)
      .toBe('background')
    await expect
      .poll(() => getSurface(screen.container, 'browser').dataset.activity)
      .toBe('foreground')
    expect(lifecycleCount('mount', 'terminal')).toBe(1)
    expect(lifecycleCount('unmount', 'terminal')).toBe(0)
    expect(lifecycleCount('mount', 'browser')).toBe(1)
    expect(lifecycleCount('unmount', 'browser')).toBe(0)
  })
})

function createTestModule(
  id: RightSidebarModuleId,
  contextBinding: RightSidebarModuleDefinition['contextBinding'],
  instancePolicy: RightSidebarModuleDefinition['instancePolicy'],
  unavailablePagePolicy: RightSidebarModuleDefinition['unavailablePagePolicy']
): RightSidebarModuleDefinition {
  const titleKey =
    id === 'terminal'
      ? 'rightSidebar.terminal'
      : id === 'browser'
        ? 'rightSidebar.browser'
        : id === 'files'
          ? 'rightSidebar.files'
          : 'rightSidebar.review'
  return {
    contextBinding,
    createPage: ({ pageId, workspace }) => ({
      id: pageId,
      moduleId: id,
      title: titleKey,
      workspaceKey: contextBinding === 'global' ? null : workspace.key,
      workspacePath: contextBinding === 'global' ? undefined : workspace.path
    }),
    icon: PanelTop,
    id,
    instancePolicy,
    render: (props) => renderTestModule(id, props),
    retention: 'keep-alive',
    surfaceKind: 'react',
    titleKey,
    unavailablePagePolicy
  }
}

function renderTestModule(id: RightSidebarModuleId, props: RightSidebarModuleRenderProps) {
  if (props.availability === 'checking') {
    return <div data-testid="review-loading">checking</div>
  }
  if (props.availability === 'unavailable') return null
  return (
    <TrackedSurface
      activity={props.activity}
      isSelected={props.isSelected}
      moduleState={props.page.moduleState}
      moduleId={id}
      pageId={props.page.id}
      onOpenPage={props.onOpenPage}
      onPageUpdate={props.onPageUpdate}
      workspaceKey={props.page.workspaceKey}
    />
  )
}

function TrackedSurface({
  activity,
  isSelected,
  moduleState,
  moduleId,
  pageId,
  onOpenPage,
  onPageUpdate,
  workspaceKey
}: {
  activity: RightSidebarActivity
  isSelected: boolean
  moduleState: RightSidebarModuleRenderProps['page']['moduleState']
  moduleId: RightSidebarModuleId
  pageId: string
  onOpenPage: RightSidebarModuleRenderProps['onOpenPage']
  onPageUpdate: RightSidebarModuleRenderProps['onPageUpdate']
  workspaceKey?: string | null
}) {
  const [localState, setLocalState] = useState(0)
  surfaceRenderSpy(moduleId, activity, isSelected)
  useEffect(() => {
    surfaceLifecycleSpy('mount', moduleId)
    return () => surfaceLifecycleSpy('unmount', moduleId)
  }, [moduleId])
  return (
    <div
      data-activity={activity}
      data-page-id={pageId}
      data-browser-url={moduleState?.kind === 'browser-surface' ? moduleState.url : undefined}
      data-review-request-id={
        moduleState?.kind === 'git-review' ? String(moduleState.requestId) : undefined
      }
      data-review-file-path={moduleState?.kind === 'git-review' ? moduleState.filePath : undefined}
      data-review-scope={moduleState?.kind === 'git-review' ? moduleState.target.kind : undefined}
      data-selected={isSelected ? 'true' : 'false'}
      data-testid={`${moduleId}-surface`}
      data-workspace-key={workspaceKey ?? 'global'}
    >
      <button
        data-testid={`${moduleId}-local-state`}
        onClick={() => {
          setLocalState((current) => current + 1)
          if (moduleId === 'browser') {
            onPageUpdate({
              moduleState: {
                kind: 'browser-surface',
                surfaceId: `fixture-${pageId}`,
                url: 'https://example.com/existing'
              }
            })
          }
        }}
        type="button"
      >
        {localState}
      </button>
      {moduleId === 'files' && (
        <>
          <button
            data-testid="files-open-readme"
            onClick={() => onOpenPage(createFilePreviewRequest('README.md'))}
            type="button"
          >
            README.md
          </button>
          <button
            data-testid="files-open-index"
            onClick={() => onOpenPage(createFilePreviewRequest('src/index.ts'))}
            type="button"
          >
            index.ts
          </button>
        </>
      )}
    </div>
  )
}

function createFilePreviewRequest(path: string) {
  return {
    disposition: 'preview' as const,
    moduleState: { kind: 'workspace-file' as const, path, tabState: 'transient' as const },
    resourceKey: `workspace-file:${path}`,
    title: path.split('/').at(-1) ?? path
  }
}

function clickTestButton(element: Element): void {
  if (!(element instanceof HTMLButtonElement)) throw new Error('Expected a test button')
  element.click()
}

function workspaceProps(workspaceKey: string, workspaceName: string, workspacePath: string) {
  return { workspaceKey, workspaceName, workspacePath }
}

function gitCapability(
  workspace: ReturnType<typeof workspaceProps>,
  status: 'checking' | 'available' | 'unavailable'
): RightSidebarCapabilities {
  return {
    'git-repository': {
      contextKey: createRightSidebarWorkspaceSessionKey(
        workspace.workspaceKey,
        workspace.workspacePath
      ),
      identity: status === 'available' ? `repository:${workspace.workspaceKey}` : undefined,
      status
    }
  }
}

async function openHomeModule(
  screen: Awaited<ReturnType<typeof render>>,
  name: string
): Promise<void> {
  await screen.getByRole('button', { name }).click()
}

async function openAdditionalModule(
  screen: Awaited<ReturnType<typeof render>>,
  name: string
): Promise<void> {
  await screen.getByRole('button', { name: 'rightSidebar.newPanel' }).click()
  await screen.getByRole('menuitem', { name }).click()
}

function getSurface(container: HTMLElement, moduleId: RightSidebarModuleId): HTMLElement {
  const surface = container.querySelector<HTMLElement>(`[data-testid="${moduleId}-surface"]`)
  if (!surface) throw new Error(`Missing ${moduleId} surface`)
  return surface
}

function getPageFrame(container: HTMLElement, moduleId: RightSidebarModuleId): HTMLElement {
  const page = getSurface(container, moduleId).closest<HTMLElement>('.right-sidebar__page')
  if (!page) throw new Error(`Missing ${moduleId} page frame`)
  return page
}

function getTabLabels(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll<HTMLElement>('.right-sidebar__tab-label')).map(
    (tab) => tab.textContent ?? ''
  )
}

function lifecycleCount(event: 'mount' | 'unmount', moduleId: RightSidebarModuleId): number {
  return surfaceLifecycleSpy.mock.calls.filter(
    ([recordedEvent, recordedModuleId]) => recordedEvent === event && recordedModuleId === moduleId
  ).length
}

function renderCount(moduleId: RightSidebarModuleId): number {
  return surfaceRenderSpy.mock.calls.filter(([recordedModuleId]) => recordedModuleId === moduleId)
    .length
}
