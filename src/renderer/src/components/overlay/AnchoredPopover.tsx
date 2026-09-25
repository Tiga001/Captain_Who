import { useLayoutEffect, useRef, type ReactNode, type RefObject } from 'react'
import { createPortal } from 'react-dom'
import './AnchoredPopover.css'

interface AnchoredPopoverProps {
  align?: 'start' | 'end'
  anchorRef: RefObject<HTMLElement | null>
  children: ReactNode
  className?: string
  enabled: boolean
  matchAnchorWidth?: boolean
  onClose: () => void
  placement?: 'auto' | 'top' | 'bottom'
  popoverRef: RefObject<HTMLDivElement | null>
  returnFocusRef?: RefObject<HTMLElement | null>
}

const VIEWPORT_MARGIN = 8
const ANCHOR_GAP = 8
/** Prefer `placement="top"` only when the menu would remain usable above the anchor. */
const MIN_PREFERRED_SPACE = 120

/** Keeps interactive composer menus outside a scrolling or hidden workspace surface. */
export function AnchoredPopover({
  align = 'start',
  anchorRef,
  children,
  className,
  enabled,
  matchAnchorWidth = false,
  onClose,
  placement = 'auto',
  popoverRef,
  returnFocusRef
}: AnchoredPopoverProps): ReactNode {
  const closeRef = useRef(onClose)
  useLayoutEffect(() => {
    closeRef.current = onClose
  }, [onClose])

  useLayoutEffect(() => {
    if (!enabled) return
    const anchor = anchorRef.current
    const popover = popoverRef.current
    if (!anchor || !popover) return
    popover.style.setProperty(
      '-webkit-font-smoothing',
      getComputedStyle(anchor).getPropertyValue('-webkit-font-smoothing')
    )
    let frame = 0

    const updatePosition = () => {
      const rect = getVisibleAnchorRect(anchor)
      if (!rect || document.visibilityState === 'hidden') {
        closeRef.current()
        return
      }
      if (matchAnchorWidth) popover.style.width = `${rect.width}px`
      const width = popover.getBoundingClientRect().width
      // The child's own maximum height (for example, the model picker's five rows) remains
      // intact. The outer viewport supplies a second scroll boundary for very short windows.
      const height = popover.scrollHeight
      const above = Math.max(0, rect.top - ANCHOR_GAP - VIEWPORT_MARGIN)
      const below = Math.max(0, window.innerHeight - rect.bottom - ANCHOR_GAP - VIEWPORT_MARGIN)
      const openAbove =
        above > 0 &&
        !(placement === 'bottom' && below >= Math.min(height, MIN_PREFERRED_SPACE)) &&
        (placement === 'top' && above >= Math.min(height, MIN_PREFERRED_SPACE)
          ? true
          : above >= height || above >= below)
      const maxHeight = openAbove ? above : below
      const left = align === 'end' ? rect.right - width : rect.left
      popover.style.left = `${Math.max(VIEWPORT_MARGIN, Math.min(left, window.innerWidth - width - VIEWPORT_MARGIN))}px`
      popover.style.top = `${openAbove ? rect.top - ANCHOR_GAP - Math.min(height, maxHeight) : rect.bottom + ANCHOR_GAP}px`
      popover.style.maxHeight = `${maxHeight}px`
      popover.style.opacity = '1'
      popover.dataset.placement = openAbove ? 'top' : 'bottom'
    }
    const schedulePosition = () => {
      window.cancelAnimationFrame(frame)
      // Composer textareas can resize in a layout/effect pass immediately after the anchor
      // changes. Wait one paint for that resize and one more for the resulting grid reflow
      // before reading rectangles, otherwise a portal menu can sit against the previous height.
      frame = window.requestAnimationFrame(() => {
        frame = window.requestAnimationFrame(updatePosition)
      })
    }
    const handleScroll = (event: Event) => {
      if (event.target instanceof Node && popover.contains(event.target)) return
      schedulePosition()
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return
      event.preventDefault()
      closeRef.current()
      ;(returnFocusRef?.current ?? anchor).focus({ preventScroll: true })
    }
    const handleVisibilityChange = () => {
      if (document.visibilityState === 'hidden') closeRef.current()
    }
    const resizeObserver = new ResizeObserver(schedulePosition)
    const visibilityObserver = new MutationObserver(schedulePosition)
    // Ancestor size and visibility changes include bottom-panel resizing, maximized sidebars,
    // inert covered workspaces, and CSS-driven page switches, even without a window resize.
    for (let element: HTMLElement | null = anchor; element; element = element.parentElement) {
      resizeObserver.observe(element)
      visibilityObserver.observe(element, { attributes: true })
    }
    resizeObserver.observe(popover)
    if (popover.firstElementChild) resizeObserver.observe(popover.firstElementChild)
    updatePosition()
    document.addEventListener('scroll', handleScroll, true)
    document.addEventListener('keydown', handleKeyDown)
    document.addEventListener('visibilitychange', handleVisibilityChange)
    window.addEventListener('resize', schedulePosition)
    return () => {
      window.cancelAnimationFrame(frame)
      resizeObserver.disconnect()
      visibilityObserver.disconnect()
      document.removeEventListener('scroll', handleScroll, true)
      document.removeEventListener('keydown', handleKeyDown)
      document.removeEventListener('visibilitychange', handleVisibilityChange)
      window.removeEventListener('resize', schedulePosition)
    }
  }, [align, anchorRef, enabled, matchAnchorWidth, placement, popoverRef, returnFocusRef])

  if (!enabled) return children
  return createPortal(
    <div className={['mc-anchored-popover', className].filter(Boolean).join(' ')} ref={popoverRef}>
      {children}
    </div>,
    document.body
  )
}

function getVisibleAnchorRect(anchor: HTMLElement): DOMRect | null {
  if (!anchor.isConnected) return null
  const rect = anchor.getBoundingClientRect()
  let left = Math.max(0, rect.left)
  let top = Math.max(0, rect.top)
  let right = Math.min(window.innerWidth, rect.right)
  let bottom = Math.min(window.innerHeight, rect.bottom)
  for (let element: HTMLElement | null = anchor; element; element = element.parentElement) {
    const style = getComputedStyle(element)
    if (
      element.hidden ||
      element.inert ||
      element.getAttribute('aria-hidden') === 'true' ||
      style.display === 'none' ||
      style.visibility === 'hidden' ||
      style.visibility === 'collapse' ||
      style.contentVisibility === 'hidden' ||
      Number(style.opacity) === 0
    ) {
      return null
    }
    if (element === anchor) continue
    const bounds = element.getBoundingClientRect()
    if (clipsOverflow(style.overflowX)) {
      left = Math.max(left, bounds.left)
      right = Math.min(right, bounds.right)
    }
    if (clipsOverflow(style.overflowY)) {
      top = Math.max(top, bounds.top)
      bottom = Math.min(bottom, bounds.bottom)
    }
  }
  // Position against the visible box. A tall composer can extend above a scrolling
  // conversation page; using the unclipped rectangle would place the menu off-screen.
  return right > left && bottom > top ? new DOMRect(left, top, right - left, bottom - top) : null
}

function clipsOverflow(value: string): boolean {
  return value === 'auto' || value === 'hidden' || value === 'clip' || value === 'scroll'
}
