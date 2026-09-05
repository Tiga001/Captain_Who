export const BOTTOM_PANEL_DEFAULT_HEIGHT = 280
export const BOTTOM_PANEL_MIN_HEIGHT = 165

export interface BottomPanelResizeMetrics {
  height: number
  minimum: number
  maximum: number
}

export function resolveBottomPanelLayout(
  shellHeight: number,
  requestedOpen: boolean,
  preferredHeight: number
) {
  const maximum = Math.max(0, Math.floor(shellHeight / 2))
  const open = requestedOpen && maximum >= BOTTOM_PANEL_MIN_HEIGHT
  const height = Math.min(maximum, Math.max(BOTTOM_PANEL_MIN_HEIGHT, preferredHeight))
  return { open, height, maximum }
}
