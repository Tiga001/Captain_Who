import {
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject
} from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import {
  calculateSidebarRawWidth,
  resolveSidebarResizeDragIntent,
  type SidebarResizeMetrics,
  type SidebarSide
} from '../../lib/sidebarResize'

interface ResizeHandleProps {
  metrics: SidebarResizeMetrics
  onCollapse: () => void
  onResizeCommit: (side: SidebarSide, width: number) => void
  resizeTargetRef: RefObject<HTMLElement | null>
  side: SidebarSide
}

type ResizeSession = {
  currentWidth: number
  frameId: number | null
  maximum: number
  minimum: number
  pendingClientX: number
  pointerId: number
  startClientX: number
  startWidth: number
}

const KEYBOARD_RESIZE_STEP = 8
const KEYBOARD_RESIZE_LARGE_STEP = 32

function liveWidthProperty(side: SidebarSide): string {
  return side === 'left' ? '--left-panel-live-width' : '--right-panel-live-width'
}

export function calculateSidebarResizeWidth(
  side: SidebarSide,
  startWidth: number,
  startClientX: number,
  currentClientX: number,
  minimum: number,
  maximum: number
): number {
  const width = calculateSidebarRawWidth(side, startWidth, startClientX, currentClientX)
  return Math.min(Math.max(width, minimum), maximum)
}

export function ResizeHandle({
  metrics,
  onCollapse,
  onResizeCommit,
  resizeTargetRef,
  side
}: ResizeHandleProps) {
  const { t } = useFrontendConfig()
  const handleRef = useRef<HTMLDivElement>(null)
  const sessionRef = useRef<ResizeSession | null>(null)
  const property = liveWidthProperty(side)

  useEffect(() => {
    if (sessionRef.current) return
    resizeTargetRef.current?.style.removeProperty(property)
  }, [metrics.width, property, resizeTargetRef])

  useEffect(
    () => () => {
      const session = sessionRef.current
      if (session?.frameId != null) cancelAnimationFrame(session.frameId)
      sessionRef.current = null
      resizeTargetRef.current?.style.removeProperty(property)
      if (session) document.body.classList.remove('is-resizing')
    },
    [property, resizeTargetRef]
  )

  const applyVisualWidth = (width: number) => {
    resizeTargetRef.current?.style.setProperty(property, `${width}px`)
    handleRef.current?.setAttribute('aria-valuenow', String(Math.round(width)))
  }

  const widthForClientX = (session: ResizeSession, clientX: number) =>
    calculateSidebarResizeWidth(
      side,
      session.startWidth,
      session.startClientX,
      clientX,
      session.minimum,
      session.maximum
    )

  const intentForClientX = (session: ResizeSession, clientX: number) =>
    resolveSidebarResizeDragIntent(
      side,
      session.startWidth,
      session.startClientX,
      clientX,
      session.minimum,
      session.maximum
    )

  const flushVisualFrame = () => {
    const session = sessionRef.current
    if (!session) return
    session.frameId = null
    session.currentWidth = widthForClientX(session, session.pendingClientX)
    applyVisualWidth(session.currentWidth)
  }

  const scheduleVisualFrame = (clientX: number) => {
    const session = sessionRef.current
    if (!session) return
    session.pendingClientX = clientX
    if (session.frameId !== null) return
    session.frameId = requestAnimationFrame(flushVisualFrame)
  }

  const finishResize = (element: HTMLDivElement, clientX: number) => {
    const session = sessionRef.current
    if (!session) return

    if (session.frameId !== null) cancelAnimationFrame(session.frameId)
    const intent = intentForClientX(session, clientX)
    sessionRef.current = null
    document.body.classList.remove('is-resizing')

    if (element.hasPointerCapture(session.pointerId)) {
      element.releasePointerCapture(session.pointerId)
    }

    if (intent.type === 'collapse') {
      // Keep the panel at its minimum until the shared toggle commits the closed layout.
      // The handle then unmounts and its cleanup removes this transient CSS width.
      applyVisualWidth(session.minimum)
      onCollapse()
      return
    }

    const finalWidth = intent.width
    applyVisualWidth(finalWidth)

    // Keep the transient CSS width until React commits the matching preferred width.
    // This prevents unrelated streaming renders from snapping the panel back mid-drag.
    onResizeCommit(side, finalWidth)
    if (finalWidth === metrics.width) resizeTargetRef.current?.style.removeProperty(property)
  }

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !event.isPrimary || sessionRef.current) return
    event.preventDefault()

    sessionRef.current = {
      currentWidth: metrics.width,
      frameId: null,
      maximum: metrics.maximum,
      minimum: metrics.minimum,
      pendingClientX: event.clientX,
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startWidth: metrics.width
    }
    document.body.classList.add('is-resizing')
    applyVisualWidth(metrics.width)
    event.currentTarget.setPointerCapture(event.pointerId)
  }

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const session = sessionRef.current
    if (event.pointerId !== session?.pointerId) return

    if (intentForClientX(session, event.clientX).type === 'collapse') {
      finishResize(event.currentTarget, event.clientX)
      return
    }

    scheduleVisualFrame(event.clientX)
  }

  const handlePointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerId !== sessionRef.current?.pointerId) return
    finishResize(event.currentTarget, event.clientX)
  }

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    let nextWidth: number | undefined
    const step = event.shiftKey ? KEYBOARD_RESIZE_LARGE_STEP : KEYBOARD_RESIZE_STEP

    if (event.key === 'Home') nextWidth = metrics.minimum
    if (event.key === 'End') nextWidth = metrics.maximum
    if (event.key === 'ArrowLeft') {
      nextWidth = metrics.width + (side === 'left' ? -step : step)
    }
    if (event.key === 'ArrowRight') {
      nextWidth = metrics.width + (side === 'left' ? step : -step)
    }
    if (nextWidth === undefined) return

    event.preventDefault()
    onResizeCommit(side, Math.min(Math.max(nextWidth, metrics.minimum), metrics.maximum))
  }

  return (
    <div
      ref={handleRef}
      className={`resize-handle resize-handle--${side}`}
      role="separator"
      aria-label={side === 'left' ? t('app.resizeLeftSidebar') : t('app.resizeRightSidebar')}
      aria-orientation="vertical"
      aria-valuemax={Math.round(metrics.maximum)}
      aria-valuemin={Math.round(metrics.minimum)}
      aria-valuenow={Math.round(metrics.width)}
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onLostPointerCapture={(event) => {
        if (event.pointerId !== sessionRef.current?.pointerId) return
        finishResize(event.currentTarget, sessionRef.current.pendingClientX)
      }}
      onPointerCancel={(event) => {
        if (event.pointerId !== sessionRef.current?.pointerId) return
        finishResize(event.currentTarget, sessionRef.current.pendingClientX)
      }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerEnd}
    />
  )
}
