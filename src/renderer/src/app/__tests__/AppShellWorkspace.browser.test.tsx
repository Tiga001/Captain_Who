// Renderer UI regression test: settings coverage must never remount the live workspace tree.
import { useEffect, useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import {
  AppShellCoveredRegion,
  AppShellWorkspace,
  getVisibleActiveConversationId
} from '../AppShellWorkspace'
import '../../styles/global.css'

describe('AppShellWorkspace', () => {
  it('removes covered content from painting without unmounting it or losing local state', async () => {
    const lifecycleSpy = vi.fn()
    const renderWorkspace = (settingsOpen: boolean) => (
      <AppShellWorkspace className="app-shell" data-testid="workspace" settingsOpen={settingsOpen}>
        <StatefulWorkspaceChild onLifecycle={lifecycleSpy} />
      </AppShellWorkspace>
    )
    const screen = await render(renderWorkspace(false))

    await screen.getByRole('button', { name: 'count 0' }).click()
    await expect.poll(() => screen.getByRole('button', { name: 'count 1' })).toBeDefined()

    await screen.rerender(renderWorkspace(true))

    const coveredWorkspace = screen.container.querySelector<HTMLElement>(
      '[data-testid="workspace"]'
    )
    expect(coveredWorkspace?.dataset.settingsOpen).toBe('true')
    expect(coveredWorkspace?.inert).toBe(true)
    expect(coveredWorkspace?.getAttribute('aria-hidden')).toBe('true')
    expect(coveredWorkspace && getComputedStyle(coveredWorkspace).opacity).toBe('0')
    expect(coveredWorkspace && getComputedStyle(coveredWorkspace).visibility).toBe('visible')
    expect(screen.container.querySelector('button')?.textContent).toBe('count 1')
    expect(lifecycleSpy.mock.calls).toEqual([['mount']])

    await screen.rerender(renderWorkspace(false))

    const restoredWorkspace = screen.container.querySelector<HTMLElement>(
      '[data-testid="workspace"]'
    )
    expect(restoredWorkspace?.hasAttribute('data-settings-open')).toBe(false)
    expect(restoredWorkspace?.inert).toBe(false)
    expect(restoredWorkspace?.hasAttribute('aria-hidden')).toBe(false)
    expect(restoredWorkspace && getComputedStyle(restoredWorkspace).opacity).toBe('1')
    expect(restoredWorkspace && getComputedStyle(restoredWorkspace).visibility).toBe('visible')
    expect(screen.container.querySelector('button')?.textContent).toBe('count 1')
    expect(lifecycleSpy.mock.calls).toEqual([['mount']])
  })

  it('isolates only the covered chat and right-sidebar regions for the scheduled view', async () => {
    const mainLifecycle = vi.fn()
    const rightLifecycle = vi.fn()
    const renderShell = (covered: boolean) => (
      <div>
        <button type="button">scheduled navigation</button>
        <AppShellCoveredRegion
          as="main"
          covered={covered}
          className="main-panel"
          data-testid="main-workspace"
        >
          <StatefulWorkspaceChild onLifecycle={mainLifecycle} />
        </AppShellCoveredRegion>
        <AppShellCoveredRegion
          as="aside"
          covered={covered}
          className="side-panel--right"
          data-testid="right-workspace"
        >
          <StatefulWorkspaceChild onLifecycle={rightLifecycle} />
        </AppShellCoveredRegion>
        {covered ? <div data-testid="scheduled-page">scheduled page</div> : null}
      </div>
    )
    const screen = await render(renderShell(false))
    const workspaceButtons = screen.container.querySelectorAll<HTMLElement>(
      '.main-panel button, .side-panel--right button'
    )
    workspaceButtons.forEach((button) => button.click())

    await screen.rerender(renderShell(true))

    const main = screen.getByTestId('main-workspace').element() as HTMLElement
    const right = screen.getByTestId('right-workspace').element() as HTMLElement
    expect(main.inert).toBe(true)
    expect(right.inert).toBe(true)
    expect(main.getAttribute('aria-hidden')).toBe('true')
    expect(right.getAttribute('aria-hidden')).toBe('true')
    expect(main.querySelector('button')?.textContent).toBe('count 1')
    expect(right.querySelector('button')?.textContent).toBe('count 1')
    expect(mainLifecycle.mock.calls).toEqual([['mount']])
    expect(rightLifecycle.mock.calls).toEqual([['mount']])
    await expect.element(screen.getByRole('button', { name: 'scheduled navigation' })).toBeEnabled()

    await screen.rerender(renderShell(false))

    expect(main.inert).toBe(false)
    expect(right.inert).toBe(false)
    expect(main.hasAttribute('aria-hidden')).toBe(false)
    expect(right.hasAttribute('aria-hidden')).toBe(false)
    expect(mainLifecycle.mock.calls).toEqual([['mount']])
    expect(rightLifecycle.mock.calls).toEqual([['mount']])
  })

  it('suppresses only the visual conversation selection while scheduled is selected', () => {
    expect(getVisibleActiveConversationId('conversation', 'conversation-1')).toBe('conversation-1')
    expect(getVisibleActiveConversationId('scheduled', 'conversation-1')).toBeNull()
  })
})

function StatefulWorkspaceChild({ onLifecycle }: { onLifecycle: (event: string) => void }) {
  const [count, setCount] = useState(0)

  useEffect(() => {
    onLifecycle('mount')
    return () => onLifecycle('unmount')
  }, [onLifecycle])

  return (
    <button type="button" onClick={() => setCount((current) => current + 1)}>
      count {count}
    </button>
  )
}
