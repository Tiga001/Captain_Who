// Renderer UI regression test: translucent sidebars must retain the selected theme tint.
import { describe, expect, it, vi } from 'vitest'

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

import { getTranslucentSidebarOpacityPercent } from '../../features/storage/storageClient'

describe('translucent sidebar tint', () => {
  it('keeps a theme-owned tint floor across the supported transparency range', () => {
    expect(getTranslucentSidebarOpacityPercent(50)).toBe('55%')
    expect(getTranslucentSidebarOpacityPercent(54)).toBe('51%')
    expect(getTranslucentSidebarOpacityPercent(82)).toBe('26%')
    expect(getTranslucentSidebarOpacityPercent(100)).toBe('10%')
  })
})
