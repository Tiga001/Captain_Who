import { useCallback, useLayoutEffect, useMemo, useState, type RefObject } from 'react'
import {
  AUTOMATION_DRAWER_DEFAULT_WIDTH,
  AUTOMATION_DRAWER_MAX_WIDTH,
  AUTOMATION_LIST_MIN_WIDTH,
  clampAutomationDrawerWidth,
  resolveAutomationLayout,
  type AutomationDrawerResizeMetrics
} from './automationLayout'

const WIDTH_CHANGE_EPSILON = 0.5

function readElementWidth(element: HTMLElement): number {
  return element.getBoundingClientRect().width
}

export interface UseAutomationLayoutOptions {
  containerRef: RefObject<HTMLElement | null>
  drawerOpen: boolean
  initialPreferredDrawerWidth?: number
  onPreferredDrawerWidthChange?: (width: number) => void
}

export interface AutomationLayoutState {
  compact: boolean
  containerWidth: number | null
  commitDrawerWidth: (width: number) => void
  drawerCoversList: boolean
  drawerResizeMetrics: AutomationDrawerResizeMetrics | null
  drawerWidth: number
  measured: boolean
  preferredDrawerWidth: number
}

/**
 * Observes the scheduled page rather than the viewport. This matters because
 * the app's left sidebar consumes a user-controlled amount of the viewport.
 */
export function useAutomationLayout({
  containerRef,
  drawerOpen,
  initialPreferredDrawerWidth = AUTOMATION_DRAWER_DEFAULT_WIDTH,
  onPreferredDrawerWidthChange
}: UseAutomationLayoutOptions): AutomationLayoutState {
  const [containerWidth, setContainerWidth] = useState<number | null>(null)
  const [preferredDrawerWidth, setPreferredDrawerWidth] = useState(() =>
    clampAutomationDrawerWidth(initialPreferredDrawerWidth)
  )

  useLayoutEffect(() => {
    const container = containerRef.current
    if (!container) return

    const updateWidth = (nextWidth = readElementWidth(container)) => {
      setContainerWidth((currentWidth) =>
        currentWidth === null || Math.abs(currentWidth - nextWidth) > WIDTH_CHANGE_EPSILON
          ? nextWidth
          : currentWidth
      )
    }

    updateWidth()

    if (typeof ResizeObserver === 'undefined') {
      const handleWindowResize = () => updateWidth()
      window.addEventListener('resize', handleWindowResize)
      return () => window.removeEventListener('resize', handleWindowResize)
    }

    const observer = new ResizeObserver((entries) => {
      const entry = entries.find((candidate) => candidate.target === container)
      updateWidth(entry?.contentRect.width)
    })
    observer.observe(container)
    return () => observer.disconnect()
  }, [containerRef])

  const commitDrawerWidth = useCallback(
    (width: number) => {
      const nextWidth = clampAutomationDrawerWidth(width)
      setPreferredDrawerWidth(nextWidth)
      onPreferredDrawerWidthChange?.(nextWidth)
    },
    [onPreferredDrawerWidthChange]
  )

  const resolved = useMemo(
    () =>
      resolveAutomationLayout(
        containerWidth ?? AUTOMATION_LIST_MIN_WIDTH + AUTOMATION_DRAWER_MAX_WIDTH,
        preferredDrawerWidth
      ),
    [containerWidth, preferredDrawerWidth]
  )
  const compact = drawerOpen && containerWidth !== null && resolved.compact

  return {
    compact,
    containerWidth,
    commitDrawerWidth,
    drawerCoversList: compact,
    drawerResizeMetrics:
      drawerOpen && containerWidth !== null && !compact ? resolved.drawerResizeMetrics : null,
    drawerWidth: resolved.drawerWidth,
    measured: containerWidth !== null,
    preferredDrawerWidth
  }
}
