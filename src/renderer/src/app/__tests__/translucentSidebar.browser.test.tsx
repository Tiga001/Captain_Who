// Renderer UI regression test: translucent sidebars must retain the selected theme tint.
import type { CSSProperties } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../styles/global.css'

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

import { getTranslucentSidebarOpacityPercent } from '../../features/storage/storageClient'

describe('translucent sidebar tint', () => {
  it('keeps a theme-owned tint floor across the supported transparency range', () => {
    expect(getTranslucentSidebarOpacityPercent(50)).toBe('66%')
    expect(getTranslucentSidebarOpacityPercent(54)).toBe('63%')
    expect(getTranslucentSidebarOpacityPercent(82)).toBe('44%')
    expect(getTranslucentSidebarOpacityPercent(100)).toBe('32%')
  })

  it('keeps the selected theme visible in the rendered sidebar at maximum transparency', async () => {
    const style = {
      '--left-panel-width': '240px',
      '--mc-sidebar-translucent-opacity': getTranslucentSidebarOpacityPercent(100),
      '--mc-sidebar-translucent-tint': '#DEE5DA'
    } as CSSProperties
    const screen = await render(
      <div className="app-shell" data-translucent-sidebar="true" style={style}>
        <aside className="side-panel side-panel--left" data-testid="left-sidebar" />
      </div>
    )
    const sidebar = screen.container.querySelector<HTMLElement>('[data-testid="left-sidebar"]')
    const backgroundColor = sidebar ? getComputedStyle(sidebar).backgroundColor : ''

    expect(backgroundColor).not.toBe('rgba(0, 0, 0, 0)')
    expect(readCssColorAlpha(backgroundColor)).toBeGreaterThanOrEqual(0.32)
  })
})

function readCssColorAlpha(color: string): number {
  const modernColorMatch = color.match(/\/\s*([\d.]+)\s*\)$/)
  if (modernColorMatch) {
    return Number(modernColorMatch[1])
  }

  const rgbaMatch = color.match(/^rgba\([^,]+,[^,]+,[^,]+,\s*([\d.]+)\)$/)
  return rgbaMatch ? Number(rgbaMatch[1]) : 1
}
