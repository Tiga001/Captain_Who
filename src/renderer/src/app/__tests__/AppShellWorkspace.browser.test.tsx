// Renderer UI regression test: settings coverage must never remount the live workspace tree.
import { useEffect, useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { AppShellWorkspace } from '../AppShellWorkspace'
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
