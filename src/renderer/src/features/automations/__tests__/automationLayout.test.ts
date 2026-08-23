import { describe, expect, it } from 'vitest'
import {
  AUTOMATION_DRAWER_DEFAULT_WIDTH,
  AUTOMATION_DRAWER_MAX_WIDTH,
  AUTOMATION_DRAWER_MIN_WIDTH,
  AUTOMATION_LIST_MIN_WIDTH,
  AUTOMATION_SPLIT_MIN_WIDTH,
  calculateAutomationDrawerRawWidth,
  clampAutomationDrawerWidth,
  resolveAutomationDrawerResizeWidth,
  resolveAutomationLayout
} from '../automationLayout'

describe('automation scheduled-page layout', () => {
  it('uses the scheduled container width for the compact boundary', () => {
    const compact = resolveAutomationLayout(AUTOMATION_SPLIT_MIN_WIDTH - 1, 440)
    const split = resolveAutomationLayout(AUTOMATION_SPLIT_MIN_WIDTH, 440)

    expect(compact).toMatchObject({ compact: true, containerWidth: 759 })
    expect(compact.drawerResizeMetrics).toBeNull()
    expect(split).toMatchObject({ compact: false, containerWidth: 760, drawerWidth: 400 })
    expect(split.drawerResizeMetrics).toEqual({ maximum: 400, minimum: 380, width: 400 })
  })

  it('always leaves the minimum list width in split mode', () => {
    for (const containerWidth of [760, 800, 1_000, 1_400]) {
      const layout = resolveAutomationLayout(containerWidth, AUTOMATION_DRAWER_MAX_WIDTH)
      expect(layout.compact).toBe(false)
      expect(containerWidth - layout.drawerWidth).toBeGreaterThanOrEqual(AUTOMATION_LIST_MIN_WIDTH)
    }
  })

  it('preserves the preferred width while the effective width contracts and restores it later', () => {
    const preferredWidth = 560
    const contracted = resolveAutomationLayout(800, preferredWidth)
    expect(contracted).toMatchObject({ drawerWidth: 440, preferredDrawerWidth: preferredWidth })

    const restored = resolveAutomationLayout(1_200, contracted.preferredDrawerWidth)
    expect(restored).toMatchObject({
      drawerWidth: preferredWidth,
      preferredDrawerWidth: preferredWidth
    })
  })

  it('normalizes invalid or out-of-range preferences', () => {
    expect(clampAutomationDrawerWidth(Number.NaN)).toBe(AUTOMATION_DRAWER_DEFAULT_WIDTH)
    expect(clampAutomationDrawerWidth(1)).toBe(AUTOMATION_DRAWER_MIN_WIDTH)
    expect(clampAutomationDrawerWidth(10_000)).toBe(AUTOMATION_DRAWER_MAX_WIDTH)
  })

  it('resizes from the drawer left edge without ever producing a collapse intent', () => {
    expect(calculateAutomationDrawerRawWidth(440, 600, 520)).toBe(520)
    expect(calculateAutomationDrawerRawWidth(440, 600, 680)).toBe(360)
    expect(resolveAutomationDrawerResizeWidth(440, 600, 1_000, 380, 640)).toBe(380)
    expect(resolveAutomationDrawerResizeWidth(440, 600, 200, 380, 640)).toBe(640)
  })
})
