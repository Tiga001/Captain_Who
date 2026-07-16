import {
  cloneElement,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactElement,
  type ReactNode
} from 'react'
import { createPortal } from 'react-dom'
import { computeTooltipPosition } from './tooltipPosition'
import type { TooltipPlacement } from './tooltipPosition'
import './Tooltip.css'

const DEFAULT_DELAY_MS = 350
const TOOLTIP_GAP = 8
const VIEWPORT_PADDING = 8

let activeTooltipDismiss: (() => void) | null = null
let pendingTooltipDismiss: { dismiss: () => void; owner: symbol } | null = null

export function dismissActiveTooltip(): void {
  pendingTooltipDismiss?.dismiss()
  activeTooltipDismiss?.()
}

interface TooltipProps {
  anchorClassName?: string
  children: ReactElement
  content: ReactNode
  delayMs?: number
  /** Link supplemental tooltip content to the trigger. Leave false when it repeats aria-label. */
  describeTrigger?: boolean
  preferredPlacement?: TooltipPlacement
}

/**
 * A non-interactive, portaled tooltip. Keeping the overlay outside its trigger's DOM subtree makes
 * it safe inside sticky headers and overflow-clipped panels.
 */
export function Tooltip({
  anchorClassName,
  children,
  content,
  delayMs = DEFAULT_DELAY_MS,
  describeTrigger = false,
  preferredPlacement = 'top'
}: TooltipProps): ReactNode {
  const tooltipId = useId()
  const ownerRef = useRef(Symbol('tooltip'))
  const anchorRef = useRef<HTMLSpanElement>(null)
  const tooltipRef = useRef<HTMLDivElement>(null)
  const hoverTimerRef = useRef<number | null>(null)
  const pendingScrollHandlerRef = useRef<(() => void) | null>(null)
  const pointerInsideRef = useRef(false)
  const focusInsideRef = useRef(false)
  const [isOpen, setIsOpen] = useState(false)
  const [isPositioned, setIsPositioned] = useState(false)
  const [placement, setPlacement] = useState<TooltipPlacement>(preferredPlacement)

  const clearHoverTimer = useCallback(() => {
    if (hoverTimerRef.current !== null) {
      window.clearTimeout(hoverTimerRef.current)
      hoverTimerRef.current = null
    }
    const pendingScrollHandler = pendingScrollHandlerRef.current
    if (pendingScrollHandler) {
      document.removeEventListener('scroll', pendingScrollHandler, true)
      pendingScrollHandlerRef.current = null
    }
    if (pendingTooltipDismiss?.owner === ownerRef.current) pendingTooltipDismiss = null
  }, [])

  const close = useCallback(() => {
    clearHoverTimer()
    setIsOpen(false)
    setIsPositioned(false)
  }, [clearHoverTimer])

  const open = useCallback(() => {
    pendingTooltipDismiss?.dismiss()
    clearHoverTimer()
    setIsOpen(true)
  }, [clearHoverTimer])

  const scheduleOpen = useCallback(() => {
    pendingTooltipDismiss?.dismiss()
    clearHoverTimer()
    hoverTimerRef.current = window.setTimeout(open, delayMs)
    pendingTooltipDismiss = { dismiss: clearHoverTimer, owner: ownerRef.current }
    const cancelPendingOpen = (): void => clearHoverTimer()
    pendingScrollHandlerRef.current = cancelPendingOpen
    document.addEventListener('scroll', cancelPendingOpen, true)
  }, [clearHoverTimer, delayMs, open])

  const closeWhenInactive = useCallback(() => {
    if (!pointerInsideRef.current && !focusInsideRef.current) close()
  }, [close])

  const updatePosition = useCallback(() => {
    const anchor = anchorRef.current
    const tooltip = tooltipRef.current
    if (!anchor || !tooltip) {
      close()
      return
    }

    const anchorRect = getVisibleElementRect(anchor)
    if (!anchorRect) {
      close()
      return
    }
    const tooltipRect = tooltip.getBoundingClientRect()
    const nextPosition = computeTooltipPosition({
      anchorRect,
      gap: TOOLTIP_GAP,
      padding: VIEWPORT_PADDING,
      preferredPlacement,
      tooltipHeight: tooltipRect.height,
      tooltipWidth: tooltipRect.width,
      viewportHeight: window.innerHeight,
      viewportWidth: window.innerWidth
    })

    tooltip.style.left = `${nextPosition.left}px`
    tooltip.style.top = `${nextPosition.top}px`
    setPlacement(nextPosition.placement)
    setIsPositioned(true)
  }, [close, preferredPlacement])

  useEffect(() => {
    return () => clearHoverTimer()
  }, [clearHoverTimer])

  useEffect(() => {
    if (!isOpen) return undefined

    activeTooltipDismiss?.()
    activeTooltipDismiss = close
    return () => {
      if (activeTooltipDismiss === close) activeTooltipDismiss = null
    }
  }, [close, isOpen])

  useLayoutEffect(() => {
    if (!isOpen) return undefined

    let animationFrame = window.requestAnimationFrame(updatePosition)
    const schedulePositionUpdate = (): void => {
      window.cancelAnimationFrame(animationFrame)
      animationFrame = window.requestAnimationFrame(updatePosition)
    }
    const resizeObserver = new ResizeObserver(schedulePositionUpdate)
    if (anchorRef.current) resizeObserver.observe(anchorRef.current)
    if (tooltipRef.current) resizeObserver.observe(tooltipRef.current)

    window.addEventListener('resize', schedulePositionUpdate)
    return () => {
      window.cancelAnimationFrame(animationFrame)
      resizeObserver.disconnect()
      window.removeEventListener('resize', schedulePositionUpdate)
    }
  }, [isOpen, updatePosition])

  useEffect(() => {
    if (!isOpen) return undefined

    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') close()
    }
    const handleVisibilityChange = (): void => {
      if (document.visibilityState !== 'visible') close()
    }

    document.addEventListener('keydown', handleKeyDown)
    document.addEventListener('pointerdown', close, true)
    document.addEventListener('scroll', close, true)
    document.addEventListener('visibilitychange', handleVisibilityChange)
    window.addEventListener('blur', close)
    return () => {
      document.removeEventListener('keydown', handleKeyDown)
      document.removeEventListener('pointerdown', close, true)
      document.removeEventListener('scroll', close, true)
      document.removeEventListener('visibilitychange', handleVisibilityChange)
      window.removeEventListener('blur', close)
    }
  }, [close, isOpen])

  return (
    <>
      <span
        className={['mc-tooltip-anchor', anchorClassName].filter(Boolean).join(' ')}
        ref={anchorRef}
        onBlurCapture={() => {
          focusInsideRef.current = false
          closeWhenInactive()
        }}
        onFocusCapture={() => {
          focusInsideRef.current = true
          open()
        }}
        onPointerEnter={() => {
          pointerInsideRef.current = true
          scheduleOpen()
        }}
        onPointerLeave={() => {
          pointerInsideRef.current = false
          clearHoverTimer()
          closeWhenInactive()
        }}
      >
        {describeTrigger && isOpen
          ? cloneElement(children, {
              'aria-describedby': mergeDescriptionIds(
                (children.props as { 'aria-describedby'?: string })['aria-describedby'],
                tooltipId
              )
            } as Record<string, string>)
          : children}
      </span>

      {isOpen &&
        createPortal(
          <div
            className="mc-tooltip"
            data-placement={placement}
            data-positioned={isPositioned ? 'true' : undefined}
            id={tooltipId}
            ref={tooltipRef}
            role="tooltip"
          >
            {content}
          </div>,
          document.body
        )}
    </>
  )
}

type VisibleRect = Pick<DOMRect, 'bottom' | 'height' | 'left' | 'right' | 'top' | 'width'>

function getVisibleElementRect(element: HTMLElement): VisibleRect | null {
  if (!element.isConnected) return null
  const elementStyle = window.getComputedStyle(element)
  if (elementStyle.display === 'none' || elementStyle.visibility === 'hidden') return null

  const rect = element.getBoundingClientRect()
  let top = Math.max(rect.top, 0)
  let right = Math.min(rect.right, window.innerWidth)
  let bottom = Math.min(rect.bottom, window.innerHeight)
  let left = Math.max(rect.left, 0)

  for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
    const style = window.getComputedStyle(ancestor)
    const ancestorRect = ancestor.getBoundingClientRect()
    if (clipsOverflow(style.overflowX)) {
      left = Math.max(left, ancestorRect.left)
      right = Math.min(right, ancestorRect.right)
    }
    if (clipsOverflow(style.overflowY)) {
      top = Math.max(top, ancestorRect.top)
      bottom = Math.min(bottom, ancestorRect.bottom)
    }
  }

  const width = right - left
  const height = bottom - top
  if (width <= 0 || height <= 0) return null

  const hitTarget = document.elementFromPoint(left + width / 2, top + height / 2)
  if (!hitTarget || !element.contains(hitTarget)) return null

  return { bottom, height, left, right, top, width }
}

function clipsOverflow(value: string): boolean {
  return value === 'auto' || value === 'clip' || value === 'hidden' || value === 'scroll'
}

function mergeDescriptionIds(currentIds: string | undefined, tooltipId: string): string {
  return currentIds ? `${currentIds} ${tooltipId}` : tooltipId
}
