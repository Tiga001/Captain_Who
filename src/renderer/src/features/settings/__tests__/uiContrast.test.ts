import { describe, expect, it } from 'vitest'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicLightTheme } from '../../../config/themes/classic'
import {
  getUiContrastMixPercents,
  mixTowardInk,
  normalizeUiContrast
} from '../../../config/uiContrast'

describe('uiContrast', () => {
  it('treats missing or invalid values as the current default look', () => {
    expect(normalizeUiContrast(undefined)).toBe(0)
    expect(normalizeUiContrast('40')).toBe(0)
    expect(normalizeUiContrast(Number.NaN)).toBe(0)
    expect(normalizeUiContrast(-12)).toBe(0)
    expect(normalizeUiContrast(140)).toBe(100)
    expect(normalizeUiContrast(53.6)).toBe(54)
  })

  it('leaves colors unchanged at 0 and mixes toward ink above that', () => {
    expect(getUiContrastMixPercents(0)).toEqual({ border: 0, surface: 0, text: 0 })
    expect(mixTowardInk('#8b95a1', '#3F3F46', 0)).toBe('#8b95a1')
    expect(mixTowardInk('#8b95a1', '#3F3F46', 18)).toBe(
      'color-mix(in srgb, #8b95a1 82%, #3F3F46 18%)'
    )

    const mixes = getUiContrastMixPercents(100)
    expect(mixes.surface).toBeGreaterThan(0)
    expect(mixes.text).toBeGreaterThan(mixes.surface)
    expect(mixes.border).toBeGreaterThan(mixes.text)
  })

  it('remaps muted chrome tokens without touching primary text, icons, or surfaces', () => {
    const baseline = getFrontendCssVariables(undefined, classicLightTheme)
    const boosted = getFrontendCssVariables(undefined, classicLightTheme, 80)

    expect(boosted['--mc-color-text-primary']).toBe(baseline['--mc-color-text-primary'])
    expect(boosted['--mc-color-icon-default']).toBe(baseline['--mc-color-icon-default'])
    expect(boosted['--mc-color-icon-accent']).toBe(baseline['--mc-color-icon-accent'])
    expect(boosted['--mc-color-surface-main-panel']).toBe(baseline['--mc-color-surface-main-panel'])
    expect(boosted['--mc-color-surface-card']).toBe(baseline['--mc-color-surface-card'])
    expect(boosted['--mc-color-border-error']).toBe(baseline['--mc-color-border-error'])
    expect(boosted['--mc-color-surface-muted']).toContain('color-mix')
    expect(boosted['--mc-color-surface-selected']).toContain('color-mix')
    expect(boosted['--mc-color-surface-selected-subtle']).toContain('color-mix')
    expect(boosted['--mc-color-state-hover']).toContain('color-mix')
    expect(boosted['--mc-color-state-active']).toContain('color-mix')
    expect(boosted['--mc-color-text-muted']).toContain('color-mix')
    expect(boosted['--mc-color-text-subtle']).toContain('color-mix')
    expect(boosted['--mc-color-text-secondary']).toContain('color-mix')
    expect(boosted['--mc-color-settings-content-muted']).toContain('color-mix')
    expect(boosted['--mc-color-settings-content-text']).toContain('color-mix')
    expect(boosted['--mc-color-sidebar-text-secondary']).toContain('color-mix')
    expect(boosted['--mc-color-sidebar-text-active']).toBe(baseline['--mc-color-sidebar-text-active'])
    expect(boosted['--mc-color-settings-content-title']).toBe(
      baseline['--mc-color-settings-content-title']
    )
    expect(boosted['--mc-color-icon-muted']).toContain('color-mix')
    expect(boosted['--mc-color-icon-subtle']).toContain('color-mix')
    expect(boosted['--mc-color-border-subtle']).toContain('color-mix')
    expect(boosted['--mc-color-border-default']).toContain('color-mix')
    expect(baseline['--mc-color-icon-muted']).toBe(classicLightTheme.colors.icon.muted)
    expect(baseline['--mc-color-text-muted']).toBe(classicLightTheme.colors.text.muted)
    expect(baseline['--mc-color-border-subtle']).toBe(classicLightTheme.colors.border.subtle)
  })
})
