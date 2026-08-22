export type SidebarSide = 'left' | 'right'

export type SidebarResizeMetrics = {
  maximum: number
  minimum: number
  width: number
}

export const SIDEBAR_COLLAPSE_THRESHOLD_RATIO = 0.5

export type SidebarResizeDragIntent = { type: 'collapse' } | { type: 'resize'; width: number }

export function calculateSidebarRawWidth(
  side: SidebarSide,
  startWidth: number,
  startClientX: number,
  currentClientX: number
): number {
  const pointerDelta = currentClientX - startClientX
  return startWidth + (side === 'left' ? pointerDelta : -pointerDelta)
}

export function resolveSidebarResizeDragIntent(
  side: SidebarSide,
  startWidth: number,
  startClientX: number,
  currentClientX: number,
  minimum: number,
  maximum: number
): SidebarResizeDragIntent {
  const rawWidth = calculateSidebarRawWidth(side, startWidth, startClientX, currentClientX)
  const collapseWidth = minimum * SIDEBAR_COLLAPSE_THRESHOLD_RATIO

  if (rawWidth <= collapseWidth) {
    return { type: 'collapse' }
  }

  return {
    type: 'resize',
    width: Math.min(Math.max(rawWidth, minimum), maximum)
  }
}
