import { useEffect } from 'react'
import { PanelTop } from 'lucide-react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  RightSidebarActivity,
  RightSidebarCapabilities,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarModuleRenderProps
} from '../rightSidebarTypes'
import { createRightSidebarWorkspaceSessionKey } from '../rightSidebarWorkspace'

const { translate } = vi.hoisted(() => ({
  translate: (key: string) => key
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: translate
  })
}))

const { RightSidebar } = await import('../../../components/sidebar/RightSidebar')
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
      moduleId={id}
      workspaceKey={props.page.workspaceKey}
    />
  )
}

function TrackedSurface({
  activity,
  isSelected,
  moduleId,
  workspaceKey
}: {
  activity: RightSidebarActivity
  isSelected: boolean
  moduleId: RightSidebarModuleId
  workspaceKey?: string | null
}) {
  surfaceRenderSpy(moduleId, activity, isSelected)
  useEffect(() => {
    surfaceLifecycleSpy('mount', moduleId)
    return () => surfaceLifecycleSpy('unmount', moduleId)
  }, [moduleId])
  return (
    <div
      data-activity={activity}
      data-selected={isSelected ? 'true' : 'false'}
      data-testid={`${moduleId}-surface`}
      data-workspace-key={workspaceKey ?? 'global'}
    />
  )
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
  return Array.from(container.querySelectorAll<HTMLElement>('.right-sidebar__tab > span')).map(
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
