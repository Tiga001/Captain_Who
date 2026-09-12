export const MIN_UI_CONTRAST = 0
export const MAX_UI_CONTRAST = 100
export const DEFAULT_UI_CONTRAST = 0

const TEXT_MIX_AT_MAX = 42
const BORDER_MIX_AT_MAX = 55

export function normalizeUiContrast(value: unknown): number {
  const numericValue =
    typeof value === 'number' && Number.isFinite(value)
      ? Math.round(value)
      : DEFAULT_UI_CONTRAST
  return Math.min(MAX_UI_CONTRAST, Math.max(MIN_UI_CONTRAST, numericValue))
}

export function getUiContrastShape(contrast: unknown): number {
  const normalized = normalizeUiContrast(contrast) / 100
  return normalized ** 1.5
}

export function mixTowardInk(color: string, ink: string, mixPercent: number): string {
  if (mixPercent <= 0) return color
  return `color-mix(in srgb, ${color} ${100 - mixPercent}%, ${ink} ${mixPercent}%)`
}

export function getUiContrastMixPercents(contrast: unknown): {
  border: number
  text: number
} {
  const shape = getUiContrastShape(contrast)
  return {
    border: Math.round(shape * BORDER_MIX_AT_MAX),
    text: Math.round(shape * TEXT_MIX_AT_MAX)
  }
}
