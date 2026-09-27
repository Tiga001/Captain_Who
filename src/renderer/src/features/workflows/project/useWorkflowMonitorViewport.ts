import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent } from 'react'
import { dismissActiveTooltip } from '../../../components/overlay/Tooltip'

const MIN_ZOOM = 0.15
const MAX_ZOOM = 2

/** A local camera only: viewing a workflow never edits its saved node positions. */
export function useWorkflowMonitorViewport(width: number, height: number) {
  const viewportRef = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState({ width: 900, height: 550 })
  const [camera, setCamera] = useState({ zoom: null as number | null, x: 0, y: 0 })
  const [dragging, setDragging] = useState(false)
  const pan = useRef<{ id: number; x: number; y: number; left: number; top: number } | null>(null)
  const fitZoom = Math.min(
    1,
    Math.max(MIN_ZOOM, Math.min(size.width / width, size.height / height))
  )
  const zoom = camera.zoom ?? fitZoom
  const left = (size.width - width * zoom) / 2 + camera.x
  const top = (size.height - height * zoom) / 2 + camera.y

  useLayoutEffect(() => {
    const element = viewportRef.current
    if (!element) return
    const observer = new ResizeObserver(() => {
      setSize({ width: element.clientWidth || 900, height: element.clientHeight || 550 })
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    const element = viewportRef.current
    if (!element) return
    const wheel = (event: WheelEvent) => {
      event.preventDefault()
      dismissActiveTooltip()
      const rect = element.getBoundingClientRect()
      const x = event.clientX - rect.left - size.width / 2
      const y = event.clientY - rect.top - size.height / 2
      const delta =
        event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? size.height : 1)
      setCamera((current) => {
        const oldZoom = current.zoom ?? fitZoom
        const nextZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, oldZoom * Math.exp(-delta * 0.002)))
        const ratio = nextZoom / oldZoom
        return { zoom: nextZoom, x: x - (x - current.x) * ratio, y: y - (y - current.y) * ratio }
      })
    }
    element.addEventListener('wheel', wheel, { passive: false })
    return () => element.removeEventListener('wheel', wheel)
  }, [fitZoom, size])

  const endPan = (event: PointerEvent<HTMLDivElement>) => {
    if (pan.current?.id !== event.pointerId) return
    pan.current = null
    setDragging(false)
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId)
  }

  return {
    viewportRef,
    size,
    zoom,
    left,
    top,
    dragging,
    zoomTo: (value: number) =>
      setCamera((current) => {
        const nextZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, value))
        const ratio = nextZoom / (current.zoom ?? fitZoom)
        return { zoom: nextZoom, x: current.x * ratio, y: current.y * ratio }
      }),
    resetView: () => setCamera({ zoom: null, x: 0, y: 0 }),
    panHandlers: {
      onPointerDown: (event: PointerEvent<HTMLDivElement>) => {
        if (pan.current || (event.button !== 0 && event.button !== 1)) return
        if (event.button === 0 && (event.target as Element).closest('button')) return
        event.preventDefault()
        dismissActiveTooltip()
        pan.current = {
          id: event.pointerId,
          x: event.clientX,
          y: event.clientY,
          left: camera.x,
          top: camera.y
        }
        event.currentTarget.setPointerCapture(event.pointerId)
        setDragging(true)
      },
      onPointerMove: (event: PointerEvent<HTMLDivElement>) => {
        const start = pan.current
        if (!start || start.id !== event.pointerId) return
        setCamera((current) => ({
          ...current,
          x: start.left + event.clientX - start.x,
          y: start.top + event.clientY - start.y
        }))
      },
      onPointerUp: endPan,
      onPointerCancel: endPan,
      onLostPointerCapture: endPan
    }
  }
}
