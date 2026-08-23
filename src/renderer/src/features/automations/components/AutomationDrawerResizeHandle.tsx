import {
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject
} from 'react'
import {
  AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY,
  resolveAutomationDrawerResizeWidth,
  type AutomationDrawerResizeMetrics
} from '../automationLayout'

const KEYBOARD_RESIZE_STEP = 8
const KEYBOARD_RESIZE_LARGE_STEP = 32

interface ResizeSession {
  frameId: number | null
  maximum: number
  minimum: number
  pendingClientX: number
  pointerId: number
  startClientX: number
  startWidth: number
}

export interface AutomationDrawerResizeHandleProps {
  ariaLabel: string
  metrics: AutomationDrawerResizeMetrics
  onResizeCommit: (width: number) => void
  resizeTargetRef: RefObject<HTMLElement | null>
}

/** A non-collapsing divider dedicated to the scheduled-page drawer. */
export function AutomationDrawerResizeHandle({
  ariaLabel,
  metrics,
  onResizeCommit,
  resizeTargetRef
}: AutomationDrawerResizeHandleProps) {
  const handleRef = useRef<HTMLDivElement>(null)
  const sessionRef = useRef<ResizeSession | null>(null)

  useEffect(() => {
    if (sessionRef.current) return
    resizeTargetRef.current?.style.removeProperty(AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY)
  }, [metrics.width, resizeTargetRef])

  useEffect(
    () => () => {
      const session = sessionRef.current
      if (session?.frameId !== null && session?.frameId !== undefined) {
        cancelAnimationFrame(session.frameId)
      }
      sessionRef.current = null
      resizeTargetRef.current?.style.removeProperty(AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY)
      if (session) document.body.classList.remove('is-resizing')
    },
    [resizeTargetRef]
  )

  const resolveWidth = (session: ResizeSession, clientX: number) =>
    resolveAutomationDrawerResizeWidth(
      session.startWidth,
      session.startClientX,
      clientX,
      session.minimum,
      session.maximum
    )

  const applyVisualWidth = (width: number) => {
    resizeTargetRef.current?.style.setProperty(AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY, `${width}px`)
    handleRef.current?.setAttribute('aria-valuenow', String(Math.round(width)))
  }

  const flushVisualFrame = () => {
    const session = sessionRef.current
    if (!session) return
    session.frameId = null
    applyVisualWidth(resolveWidth(session, session.pendingClientX))
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
    const finalWidth = resolveWidth(session, clientX)
    sessionRef.current = null
    document.body.classList.remove('is-resizing')

    if (element.hasPointerCapture(session.pointerId)) {
      element.releasePointerCapture(session.pointerId)
    }

    applyVisualWidth(finalWidth)
    onResizeCommit(finalWidth)
    if (finalWidth === metrics.width) {
      resizeTargetRef.current?.style.removeProperty(AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY)
    }
  }

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !event.isPrimary || sessionRef.current) return
    event.preventDefault()
    sessionRef.current = {
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
    if (event.pointerId !== sessionRef.current?.pointerId) return
    scheduleVisualFrame(event.clientX)
  }

  const handlePointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerId !== sessionRef.current?.pointerId) return
    finishResize(event.currentTarget, event.clientX)
  }

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? KEYBOARD_RESIZE_LARGE_STEP : KEYBOARD_RESIZE_STEP
    let nextWidth: number | undefined

    if (event.key === 'Home') nextWidth = metrics.minimum
    if (event.key === 'End') nextWidth = metrics.maximum
    if (event.key === 'ArrowLeft') nextWidth = metrics.width + step
    if (event.key === 'ArrowRight') nextWidth = metrics.width - step
    if (nextWidth === undefined) return

    event.preventDefault()
    onResizeCommit(Math.min(Math.max(nextWidth, metrics.minimum), metrics.maximum))
  }

  return (
    <div
      ref={handleRef}
      className="automation-drawer-resize-handle"
      role="separator"
      aria-label={ariaLabel}
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
