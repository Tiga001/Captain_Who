import { useEffect, useState } from 'react'
import type { ComponentProps } from 'react'
import { TerminalSquare } from 'lucide-react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  RightSidebarModuleDefinition,
  RightSidebarModuleRenderProps
} from '../rightSidebarTypes'

const { translate } = vi.hoisted(() => ({ translate: (key: string) => key }))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: translate })
}))
const { RightSidebar } = await import('../RightSidebar')
const NOOP = () => undefined
const WORKFLOW = { id: 'workflow-a', name: 'Organization A', color: '#37aa99' }
const MODULES: RightSidebarModuleDefinition[] = [
  {
    id: 'terminal',
    contextBinding: 'global',
    instancePolicy: 'single',
    createPage: ({ pageId }) => ({ id: pageId, moduleId: 'terminal', title: 'Terminal' }),
    icon: TerminalSquare,
    retention: 'keep-alive',
    surfaceKind: 'react',
    titleKey: 'rightSidebar.terminal',
    unavailablePagePolicy: 'retain-page',
    render: () => <div>Terminal surface</div>
  }
]

function WorkflowSurface({ page, onPageUpdate, activity }: RightSidebarModuleRenderProps) {
  const [draft, setDraft] = useState(0)
  const state = page.moduleState?.kind === 'workflows' ? page.moduleState : null
  const instanceId = state?.instanceId ?? null
  useEffect(() => {
    onPageUpdate({ title: instanceId ?? 'sidebar.workflows' })
  }, [instanceId, onPageUpdate])
  return (
    <div
      data-testid="workflow-surface"
      data-instance={instanceId ?? 'home'}
      data-activity={activity}
      data-navigation={state?.navigationId}
    >
      <button onClick={() => setDraft((value) => value + 1)}>draft {draft}</button>
      <button
        onClick={() =>
          onPageUpdate({
            moduleState: { kind: 'workflows', instanceId: null, navigationId: state?.navigationId }
          })
        }
      >
        Return home
      </button>
    </div>
  )
}
function renderWorkflow(props: RightSidebarModuleRenderProps) {
  return (
    <WorkflowSurface
      key={props.page.moduleState?.kind === 'workflows' ? props.page.moduleState.navigationId : 0}
      {...props}
    />
  )
}
function sidebar(props: Partial<ComponentProps<typeof RightSidebar>> = {}) {
  return (
    <div style={{ width: 680, height: 600 }}>
      <RightSidebar
        isOpen
        isMaximized={false}
        modules={MODULES}
        onToggleMaximized={NOOP}
        renderWorkflow={renderWorkflow}
        {...props}
      />
    </div>
  )
}
function workflowSurface(container: HTMLElement) {
  return container.querySelector<HTMLElement>('[data-testid="workflow-surface"]')!
}
function closeWorkflowTab(container: HTMLElement) {
  const tab = [...container.querySelectorAll<HTMLElement>('.right-sidebar__tab-shell')].find(
    (element) =>
      element.textContent?.includes('workflow') ||
      element.textContent?.includes('sidebar.workflows')
  )!
  tab.querySelector<HTMLButtonElement>('.right-sidebar__tab-close')!.click()
}

afterEach(() => vi.restoreAllMocks())

describe('Organization right sidebar', () => {
  it('shows its entry only for organization members and preserves the global tab across conversation and workspace changes', async () => {
    const screen = await render(sidebar({ activeConversationId: 'outside' }))
    expect(screen.container.textContent).not.toContain('sidebar.workflows')
    await screen.rerender(
      sidebar({
        activeConversationId: 'member',
        activeWorkflow: WORKFLOW,
        workspaceKey: 'project-a'
      })
    )
    await screen.getByRole('button', { name: 'sidebar.workflows', exact: true }).click()
    await expect.poll(() => workflowSurface(screen.container)?.dataset.instance).toBe('workflow-a')
    await screen.getByRole('button', { name: 'draft 0' }).click()
    const surface = workflowSurface(screen.container)

    await screen.rerender(
      sidebar({
        activeConversationId: 'outside',
        workspaceKey: 'project-b',
        workspaceKeys: ['project-b']
      })
    )
    expect(workflowSurface(screen.container)).toBe(surface)
    expect(surface.textContent).toContain('draft 1')
    expect(surface.dataset.instance).toBe('workflow-a')
    await screen.getByRole('button', { name: 'rightSidebar.newPanel' }).click()
    expect(document.querySelector('.right-sidebar__module-menu')?.textContent).not.toContain(
      'sidebar.workflows'
    )
    await screen.getByRole('menuitem', { name: 'rightSidebar.terminal', exact: true }).click()
    expect(workflowSurface(screen.container)).toBe(surface)
    expect(surface.dataset.activity).toBe('background')
    await screen.getByRole('tab', { name: 'workflow-a', exact: true }).click()
    expect(surface.dataset.activity).toBe('foreground')
    expect(surface.textContent).toContain('draft 1')
  })

  it('opens the home externally without membership and reuses one tab for every organization route', async () => {
    const screen = await render(
      sidebar({ workflowNavigationRequest: { instanceId: null, requestId: 1 } })
    )
    await expect.poll(() => workflowSurface(screen.container)?.dataset.instance).toBe('home')
    const firstNavigation = workflowSurface(screen.container).dataset.navigation
    await screen.rerender(
      sidebar({ workflowNavigationRequest: { instanceId: 'workflow-b', requestId: 2 } })
    )
    await expect.poll(() => workflowSurface(screen.container)?.dataset.instance).toBe('workflow-b')
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
    expect(workflowSurface(screen.container).dataset.navigation).not.toBe(firstNavigation)
    await screen.getByRole('button', { name: 'Return home' }).click()
    await expect
      .poll(() => screen.container.querySelector('[role="tab"]')?.textContent)
      .toBe('sidebar.workflows')
    await screen.getByRole('button', { name: 'draft 0' }).click()
    await screen.rerender(
      sidebar({ workflowNavigationRequest: { instanceId: null, requestId: 3 } })
    )
    await expect.poll(() => workflowSurface(screen.container)?.textContent).toContain('draft 0')
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
    await screen.rerender(
      sidebar({ workflowNavigationRequest: { instanceId: 'stale', requestId: 2 } })
    )
    expect(workflowSurface(screen.container).dataset.instance).toBe('home')
  })

  it('guards replacement and closure while rejecting superseded confirmations and allowing ordinary tab switches', async () => {
    const confirmations: Array<() => void> = []
    const guard = vi.fn((proceed: () => void) => confirmations.push(proceed))
    const props = { activeWorkflow: WORKFLOW, onBeforeWorkflowNavigate: guard }
    const screen = await render(
      sidebar({ ...props, workflowNavigationRequest: { instanceId: 'workflow-a', requestId: 1 } })
    )
    await expect.poll(() => workflowSurface(screen.container)?.dataset.instance).toBe('workflow-a')
    expect(guard).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'draft 0' }).click()

    await screen.rerender(
      sidebar({ ...props, workflowNavigationRequest: { instanceId: 'workflow-b', requestId: 2 } })
    )
    expect(guard).toHaveBeenCalledTimes(1)
    expect(workflowSurface(screen.container).dataset.instance).toBe('workflow-a')
    expect(workflowSurface(screen.container).textContent).toContain('draft 1')
    await screen.rerender(
      sidebar({ ...props, workflowNavigationRequest: { instanceId: null, requestId: 3 } })
    )
    confirmations[0]()
    expect(workflowSurface(screen.container).dataset.instance).toBe('workflow-a')
    confirmations[1]()
    await expect.poll(() => workflowSurface(screen.container)?.dataset.instance).toBe('home')

    await screen.getByRole('button', { name: 'rightSidebar.newPanel' }).click()
    await screen.getByRole('menuitem', { name: 'rightSidebar.terminal', exact: true }).click()
    await screen.getByRole('tab', { name: 'sidebar.workflows', exact: true }).click()
    expect(guard).toHaveBeenCalledTimes(2)
    closeWorkflowTab(screen.container)
    expect(guard).toHaveBeenCalledTimes(3)
    expect(workflowSurface(screen.container)).not.toBeNull()
    confirmations[2]()
    await expect.poll(() => workflowSurface(screen.container)).toBeNull()
    confirmations[2]()
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
  })

  it('reports organization visibility without changing its route when the sidebar is hidden', async () => {
    const visibility = vi.fn()
    const props = {
      onWorkflowVisibilityChange: visibility,
      workflowNavigationRequest: { instanceId: 'workflow-a', requestId: 1 }
    }
    const screen = await render(sidebar(props))
    await expect.poll(() => visibility).toHaveBeenLastCalledWith(true)
    const surface = workflowSurface(screen.container)
    await screen.rerender(sidebar({ ...props, isOpen: false }))
    expect(visibility).toHaveBeenLastCalledWith(false)
    expect(workflowSurface(screen.container)).toBe(surface)
    await screen.rerender(sidebar({ ...props, isWorkspaceVisible: false }))
    expect(visibility).toHaveBeenLastCalledWith(false)
    await screen.rerender(sidebar(props))
    expect(visibility).toHaveBeenLastCalledWith(true)
    expect(workflowSurface(screen.container)).toBe(surface)
  })
})
