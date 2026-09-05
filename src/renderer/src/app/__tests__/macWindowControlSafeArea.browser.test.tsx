// Renderer UI regression test: collapsed navigation must not overlap macOS traffic lights.
import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import type { CSSProperties } from 'react'
import '../../styles/global.css'
import '../../features/rightSidebar/RightSidebar.css'
import '../../features/automations/ScheduledPage.css'

// Match the shell's resolved geometry when navigation is closed.
const shellStyle = {
  '--left-panel-width': '0px',
  '--right-panel-width': '0px',
  height: 720,
  width: 1280
} as CSSProperties

function MaximizedScheduledDrawer() {
  return (
    <section
      className="scheduled-page"
      data-drawer-maximized="true"
      data-layout="split"
      data-testid="scheduled-page"
    >
      <aside className="automation-drawer">
        <header className="automation-drawer__header" data-testid="automation-drawer-header">
          <div className="automation-drawer__heading" data-testid="automation-drawer-heading">
            New task
          </div>
        </header>
      </aside>
    </section>
  )
}

describe('macOS window control safe area', () => {
  it('keeps chat and maximized right-sidebar controls clear when the left sidebar is closed', async () => {
    const screen = await render(
      <div
        className="app-shell"
        data-left-open="false"
        data-macos-window-controls="true"
        data-window-maximized="true"
        style={shellStyle}
      >
        <main className="main-panel">
          <div className="main-panel__toolbar">
            <button className="panel-toggle panel-toggle--left" data-testid="left-toggle" />
          </div>
        </main>
        <aside className="right-sidebar right-sidebar--maximized">
          <header className="right-sidebar__toolbar" data-testid="right-toolbar" />
        </aside>
        <MaximizedScheduledDrawer />
      </div>
    )

    const leftToggle = screen.container.querySelector<HTMLElement>('[data-testid="left-toggle"]')
    const rightToolbar = screen.container.querySelector<HTMLElement>(
      '[data-testid="right-toolbar"]'
    )
    const drawerHeading = screen.getByTestId('automation-drawer-heading').element()

    expect(leftToggle && getComputedStyle(leftToggle).left).toBe('84px')
    expect(rightToolbar && getComputedStyle(rightToolbar).paddingLeft).toBe('84px')
    expect(leftToggle!.getBoundingClientRect().right).toBeLessThanOrEqual(
      drawerHeading.getBoundingClientRect().left
    )
  })

  it('does not reserve the macOS traffic-light area on other platforms', async () => {
    const screen = await render(
      <div className="app-shell" data-left-open="false" style={shellStyle}>
        <main className="main-panel">
          <div className="main-panel__toolbar">
            <button className="panel-toggle panel-toggle--left" data-testid="left-toggle" />
          </div>
        </main>
        <aside className="right-sidebar right-sidebar--maximized">
          <header className="right-sidebar__toolbar" data-testid="right-toolbar" />
        </aside>
        <MaximizedScheduledDrawer />
      </div>
    )

    const leftToggle = screen.container.querySelector<HTMLElement>('[data-testid="left-toggle"]')
    const rightToolbar = screen.container.querySelector<HTMLElement>(
      '[data-testid="right-toolbar"]'
    )
    const drawerHeading = screen.getByTestId('automation-drawer-heading').element()

    expect(leftToggle && getComputedStyle(leftToggle).left).toBe('20px')
    expect(rightToolbar && getComputedStyle(rightToolbar).paddingLeft).toBe('14px')
    expect(leftToggle!.getBoundingClientRect().right).toBeLessThanOrEqual(
      drawerHeading.getBoundingClientRect().left
    )
  })
})
