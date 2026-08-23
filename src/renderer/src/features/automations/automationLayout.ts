export const AUTOMATION_DRAWER_DEFAULT_WIDTH = 440
export const AUTOMATION_DRAWER_MIN_WIDTH = 380
export const AUTOMATION_DRAWER_MAX_WIDTH = 640
export const AUTOMATION_LIST_MIN_WIDTH = 360

// Keep a small buffer beyond the two absolute panel minima. Besides avoiding a
// cramped split layout, this gives fractional ResizeObserver measurements a
// stable boundary above the panels' exact 740px minimum sum.
export const AUTOMATION_SPLIT_MIN_WIDTH = 760

export const AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY = '--automation-drawer-live-width'

export interface AutomationDrawerResizeMetrics {
  maximum: number
  minimum: number
  width: number
}

export interface AutomationLayoutResolution {
  compact: boolean
  containerWidth: number
  drawerResizeMetrics: AutomationDrawerResizeMetrics | null
  drawerWidth: number
  preferredDrawerWidth: number
}

function normalizeContainerWidth(width: number): number {
  return Number.isFinite(width) ? Math.max(0, width) : 0
}

export function clampAutomationDrawerWidth(width: number): number {
  const finiteWidth = Number.isFinite(width) ? width : AUTOMATION_DRAWER_DEFAULT_WIDTH
  return Math.min(Math.max(finiteWidth, AUTOMATION_DRAWER_MIN_WIDTH), AUTOMATION_DRAWER_MAX_WIDTH)
}

export function calculateAutomationDrawerRawWidth(
  startWidth: number,
  startClientX: number,
  currentClientX: number
): number {
  return startWidth + startClientX - currentClientX
}

export function resolveAutomationDrawerResizeWidth(
  startWidth: number,
  startClientX: number,
  currentClientX: number,
  minimum: number,
  maximum: number
): number {
  const rawWidth = calculateAutomationDrawerRawWidth(startWidth, startClientX, currentClientX)
  return Math.min(Math.max(rawWidth, minimum), maximum)
}

/**
 * Resolves the drawer against the width of the scheduled page itself. The
 * preferred width is kept separate from the effective width so a temporary
 * container contraction never destroys the user's wider preference.
 */
export function resolveAutomationLayout(
  containerWidth: number,
  preferredDrawerWidth: number
): AutomationLayoutResolution {
  const normalizedContainerWidth = normalizeContainerWidth(containerWidth)
  const normalizedPreferredWidth = clampAutomationDrawerWidth(preferredDrawerWidth)
  const compact = normalizedContainerWidth < AUTOMATION_SPLIT_MIN_WIDTH

  if (compact) {
    return {
      compact: true,
      containerWidth: normalizedContainerWidth,
      drawerResizeMetrics: null,
      drawerWidth: normalizedContainerWidth,
      preferredDrawerWidth: normalizedPreferredWidth
    }
  }

  const maximum = Math.min(
    AUTOMATION_DRAWER_MAX_WIDTH,
    normalizedContainerWidth - AUTOMATION_LIST_MIN_WIDTH
  )
  const drawerWidth = Math.min(normalizedPreferredWidth, maximum)

  return {
    compact: false,
    containerWidth: normalizedContainerWidth,
    drawerResizeMetrics: {
      maximum,
      minimum: AUTOMATION_DRAWER_MIN_WIDTH,
      width: drawerWidth
    },
    drawerWidth,
    preferredDrawerWidth: normalizedPreferredWidth
  }
}
