import { describe, expect, it } from 'vitest'
import { resolveBottomPanelLayout } from '../bottomPanelLayout'

describe('bottom panel height bounds', () => {
  it('permits the compact 165px floor while keeping the maximum at half the shell height', () => {
    expect(resolveBottomPanelLayout(900, true, 100)).toEqual({
      open: true,
      height: 165,
      maximum: 450
    })
    expect(resolveBottomPanelLayout(900, true, 700)).toEqual({
      open: true,
      height: 450,
      maximum: 450
    })
  })

  it('automatically hides only below the minimum usable shell height and restores the preference', () => {
    expect(resolveBottomPanelLayout(400, true, 280)).toEqual({
      open: true,
      height: 200,
      maximum: 200
    })
    expect(resolveBottomPanelLayout(330, true, 280)).toEqual({
      open: true,
      height: 165,
      maximum: 165
    })
    expect(resolveBottomPanelLayout(329, true, 280).open).toBe(false)
    expect(resolveBottomPanelLayout(900, true, 280).height).toBe(280)
    expect(resolveBottomPanelLayout(900, false, 280).open).toBe(false)
  })
})
