import { describe, expect, it } from 'vitest'
import {
  getRightMaximumWidth,
  getSidebarResizeMaximum,
  resolveShellLayout
} from '../shellLayoutPolicy'

const wideIntent = {
  shellWidth: 1920,
  leftRequestedOpen: true,
  rightRequestedOpen: true,
  leftPreferredWidth: 288,
  rightPreferredWidth: 1000
} as const

describe('shellLayoutPolicy', () => {
  it('allows a wide right sidebar while preserving the center minimum width', () => {
    const layout = resolveShellLayout(wideIntent)

    expect(layout.leftOpen).toBe(true)
    expect(layout.rightOpen).toBe(true)
    expect(layout.rightWidth).toBe(1000)
    expect(layout.rightWidth).toBeGreaterThan(560)
    expect(layout.centerWidth).toBe(632)
  })

  it('uses a viewport-aware right maximum with an absolute cap', () => {
    expect(getRightMaximumWidth(800)).toBe(576)
    expect(getRightMaximumWidth(1920)).toBe(1200)
    expect(getRightMaximumWidth(4000)).toBe(1200)

    expect(getSidebarResizeMaximum('right', 1920, 288)).toBe(1152)
    expect(getSidebarResizeMaximum('left', 1920, 360)).toBe(420)
  })

  it('shrinks the right sidebar first during natural window resizing', () => {
    const layout = resolveShellLayout({
      ...wideIntent,
      shellWidth: 1440
    })

    expect(layout.leftWidth).toBe(288)
    expect(layout.rightWidth).toBe(672)
    expect(layout.centerWidth).toBe(480)
  })

  it('temporarily suppresses the other sidebar when an explicitly opened side needs space', () => {
    const intent = {
      ...wideIntent,
      shellWidth: 900,
      preferredSide: 'right' as const
    }
    const constrained = resolveShellLayout(intent)

    expect(constrained.leftOpen).toBe(false)
    expect(constrained.rightOpen).toBe(true)
    expect(constrained.centerWidth).toBeGreaterThanOrEqual(480)

    const restored = resolveShellLayout({ ...intent, shellWidth: 1600 })
    expect(restored.leftOpen).toBe(true)
    expect(restored.rightOpen).toBe(true)
  })

  it('does not restore a sidebar that the user requested closed', () => {
    const layout = resolveShellLayout({
      ...wideIntent,
      shellWidth: 1920,
      rightRequestedOpen: false
    })

    expect(layout.leftOpen).toBe(true)
    expect(layout.rightOpen).toBe(false)
    expect(layout.centerWidth).toBe(1632)
  })

  it('hides sidebars when the shell cannot preserve their minimum and the center minimum', () => {
    const layout = resolveShellLayout({
      ...wideIntent,
      shellWidth: 520,
      rightRequestedOpen: false
    })

    expect(layout.leftOpen).toBe(false)
    expect(layout.rightOpen).toBe(false)
    expect(layout.centerWidth).toBe(520)
  })
})
