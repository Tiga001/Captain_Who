import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
  type RefObject
} from 'react'
import { createPortal } from 'react-dom'

export type GitReviewMenuPlacement = 'bottom-start' | 'bottom-end' | 'side-start'

interface GitReviewMenuPortalProps {
  anchorRef: RefObject<HTMLElement | null>
  ariaLabel?: string
  autoFocus?: 'first' | 'selected' | 'none'
  children: ReactNode
  className?: string
  onArrowLeft?: () => void
  onEscape: () => void
  onPointerEnter?: () => void
  placement: GitReviewMenuPlacement
}

const VIEWPORT_MARGIN = 8
const MENU_GAP = 6

export function GitReviewMenuPortal({
  anchorRef,
  ariaLabel,
  autoFocus = 'none',
  children,
  className,
  onArrowLeft,
  onEscape,
  onPointerEnter,
  placement
}: GitReviewMenuPortalProps): ReactNode {
  const menuRef = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState<CSSProperties>({ visibility: 'hidden' })

  const updatePosition = useCallback((): void => {
    const anchor = anchorRef.current
    const menu = menuRef.current
    if (!anchor || !menu || !anchor.isConnected) return

    const anchorRect = anchor.getBoundingClientRect()
    const menuRect = menu.getBoundingClientRect()
    const maxLeft = Math.max(VIEWPORT_MARGIN, window.innerWidth - menuRect.width - VIEWPORT_MARGIN)
    const maxTop = Math.max(VIEWPORT_MARGIN, window.innerHeight - menuRect.height - VIEWPORT_MARGIN)
    let left: number
    let top: number

    if (placement === 'side-start') {
      const rightSpace = window.innerWidth - anchorRect.right - VIEWPORT_MARGIN
      const leftSpace = anchorRect.left - VIEWPORT_MARGIN
      left =
        rightSpace >= menuRect.width + MENU_GAP || rightSpace >= leftSpace
          ? anchorRect.right + MENU_GAP
          : anchorRect.left - menuRect.width - MENU_GAP
      top = anchorRect.top
    } else {
      const belowSpace = window.innerHeight - anchorRect.bottom - VIEWPORT_MARGIN
      const aboveSpace = anchorRect.top - VIEWPORT_MARGIN
      top =
        belowSpace >= menuRect.height + MENU_GAP || belowSpace >= aboveSpace
          ? anchorRect.bottom + MENU_GAP
          : anchorRect.top - menuRect.height - MENU_GAP
      left = placement === 'bottom-end' ? anchorRect.right - menuRect.width : anchorRect.left
    }

    const next = {
      left: Math.round(Math.min(maxLeft, Math.max(VIEWPORT_MARGIN, left))),
      top: Math.round(Math.min(maxTop, Math.max(VIEWPORT_MARGIN, top))),
      visibility: 'visible'
    } satisfies CSSProperties
    setPosition((current) =>
      current.left === next.left &&
      current.top === next.top &&
      current.visibility === next.visibility
        ? current
        : next
    )
  }, [anchorRef, placement])

  useLayoutEffect(() => {
    let animationFrame = window.requestAnimationFrame(updatePosition)
    const scheduleUpdate = (): void => {
      window.cancelAnimationFrame(animationFrame)
      animationFrame = window.requestAnimationFrame(updatePosition)
    }
    const observer =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(scheduleUpdate)
    if (anchorRef.current) observer?.observe(anchorRef.current)
    if (menuRef.current) observer?.observe(menuRef.current)
    window.addEventListener('resize', scheduleUpdate)
    window.addEventListener('scroll', scheduleUpdate, true)
    return () => {
      window.cancelAnimationFrame(animationFrame)
      observer?.disconnect()
      window.removeEventListener('resize', scheduleUpdate)
      window.removeEventListener('scroll', scheduleUpdate, true)
    }
  }, [anchorRef, updatePosition])

  useLayoutEffect(() => {
    if (autoFocus === 'none' || position.visibility !== 'visible') return undefined
    const animationFrame = window.requestAnimationFrame(() => {
      const menu = menuRef.current
      if (!menu) return
      const selected =
        autoFocus === 'selected'
          ? menu.querySelector<HTMLElement>(
              '[role^="menuitem"][aria-checked="true"]:not(:disabled)'
            )
          : null
      selected?.focus()
      if (!selected) (menuItems(menu)[0] ?? menu).focus()
    })
    return () => window.cancelAnimationFrame(animationFrame)
  }, [autoFocus, position.visibility])

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (event.defaultPrevented) return
    const items = menuItems(event.currentTarget)
    const currentIndex = items.findIndex((item) => item === document.activeElement)
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      if (items.length === 0) return
      const direction = event.key === 'ArrowDown' ? 1 : -1
      const nextIndex =
        currentIndex < 0 ? (direction > 0 ? 0 : items.length - 1) : currentIndex + direction
      items[(nextIndex + items.length) % items.length]?.focus()
      return
    }
    if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault()
      if (items.length === 0) return
      items[event.key === 'Home' ? 0 : items.length - 1]?.focus()
      return
    }
    if (event.key === 'ArrowLeft' && onArrowLeft) {
      event.preventDefault()
      event.stopPropagation()
      onArrowLeft()
      return
    }
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      onEscape()
    }
  }

  return createPortal(
    <div
      aria-label={ariaLabel}
      className={['git-review__menu', 'git-review__menu--portal', className]
        .filter(Boolean)
        .join(' ')}
      data-git-review-menu-portal="true"
      onKeyDown={handleKeyDown}
      onPointerEnter={onPointerEnter}
      ref={menuRef}
      role="menu"
      style={position}
      tabIndex={-1}
    >
      {children}
    </div>,
    document.body
  )
}

function menuItems(menu: HTMLElement): HTMLElement[] {
  return Array.from(
    menu.querySelectorAll<HTMLElement>(
      '[role="menuitem"]:not(:disabled), [role="menuitemradio"]:not(:disabled), [role="menuitemcheckbox"]:not(:disabled)'
    )
  ).filter((item) => item.closest('[role="menu"]') === menu)
}
