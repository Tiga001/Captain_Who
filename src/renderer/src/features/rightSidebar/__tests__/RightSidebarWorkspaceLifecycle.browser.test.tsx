import { useEffect } from 'react'
import { PanelTop } from 'lucide-react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  RightSidebarCapabilities,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarModuleRenderProps
} from '../rightSidebarTypes'
import { createRightSidebarWorkspaceSessionKey } from '../rightSidebarWorkspace'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => key
  })
}))

const { RightSidebar } = await import('../../../components/sidebar/RightSidebar')
const surfaceLifecycleSpy = vi.fn()

const MODULES: RightSidebarModuleDefinition[] = [
  createTestModule('terminal', 'pinned-to-creation-workspace', 'multiple', 'retain-page'),
  createTestModule('browser', 'global', 'multiple', 'retain-page'),
  {
    ...createTestModule('git-review', 'follow-workspace', 'single', 'close-page'),
    requiredCapability: 'git-repository'
  }
]

beforeEach(() => {
  surfaceLifecycleSpy.mockClear()
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
        modules={MODULES}
        onToggleMaximized={() => undefined}
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

    await screen.rerender(
      <RightSidebar
        {...workspaceB}
        capabilities={gitCapability(workspaceB, 'checking')}
        isMaximized={false}
        modules={MODULES}
        onToggleMaximized={() => undefined}
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
        modules={MODULES}
        onToggleMaximized={() => undefined}
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

    await screen.rerender(
      <RightSidebar
        {...workspaceB}
        capabilities={gitCapability(workspaceB, 'unavailable')}
        isMaximized={false}
        modules={MODULES}
        onToggleMaximized={() => undefined}
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
        modules={MODULES}
        onToggleMaximized={() => undefined}
      />
    )
    await openHomeModule(screen, 'rightSidebar.review')
    await expect.poll(() => lifecycleCount('mount', 'git-review')).toBe(1)

    await screen.rerender(
      <RightSidebar
        {...workspace}
        capabilities={gitCapability(workspace, 'available')}
        isMaximized={false}
        modules={MODULES}
        onToggleMaximized={() => undefined}
      />
    )

    expect(lifecycleCount('mount', 'git-review')).toBe(1)
    expect(lifecycleCount('unmount', 'git-review')).toBe(0)
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
  return <TrackedSurface moduleId={id} workspaceKey={props.page.workspaceKey} />
}

function TrackedSurface({
  moduleId,
  workspaceKey
}: {
  moduleId: RightSidebarModuleId
  workspaceKey?: string | null
}) {
  useEffect(() => {
    surfaceLifecycleSpy('mount', moduleId)
    return () => surfaceLifecycleSpy('unmount', moduleId)
  }, [moduleId])
  return <div data-testid={`${moduleId}-surface`} data-workspace-key={workspaceKey ?? 'global'} />
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
