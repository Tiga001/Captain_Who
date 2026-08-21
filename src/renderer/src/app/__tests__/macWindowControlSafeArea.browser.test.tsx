// Renderer UI regression test: collapsed navigation must not overlap macOS traffic lights.
import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../styles/global.css'
import '../../features/rightSidebar/RightSidebar.css'

describe('macOS window control safe area', () => {
  it('keeps chat and maximized right-sidebar controls clear when the left sidebar is closed', async () => {
    const screen = await render(
      <div
        className="app-shell"
        data-left-open="false"
        data-macos-window-controls="true"
        data-window-maximized="true"
      >
        <main className="main-panel">
          <div className="main-panel__toolbar">
            <button className="panel-toggle panel-toggle--left" data-testid="left-toggle" />
          </div>
        </main>
        <aside className="right-sidebar right-sidebar--maximized">
          <header className="right-sidebar__toolbar" data-testid="right-toolbar" />
        </aside>
      </div>
    )

    const leftToggle = screen.container.querySelector<HTMLElement>('[data-testid="left-toggle"]')
    const rightToolbar = screen.container.querySelector<HTMLElement>(
      '[data-testid="right-toolbar"]'
    )

    expect(leftToggle && getComputedStyle(leftToggle).left).toBe('84px')
    expect(rightToolbar && getComputedStyle(rightToolbar).paddingLeft).toBe('84px')
  })

  it('does not reserve the macOS traffic-light area on other platforms', async () => {
    const screen = await render(
      <div className="app-shell" data-left-open="false">
        <main className="main-panel">
          <div className="main-panel__toolbar">
            <button className="panel-toggle panel-toggle--left" data-testid="left-toggle" />
          </div>
        </main>
        <aside className="right-sidebar right-sidebar--maximized">
          <header className="right-sidebar__toolbar" data-testid="right-toolbar" />
        </aside>
      </div>
    )

    const leftToggle = screen.container.querySelector<HTMLElement>('[data-testid="left-toggle"]')
    const rightToolbar = screen.container.querySelector<HTMLElement>(
      '[data-testid="right-toolbar"]'
    )

    expect(leftToggle && getComputedStyle(leftToggle).left).toBe('20px')
    expect(rightToolbar && getComputedStyle(rightToolbar).paddingLeft).toBe('14px')
  })
})
