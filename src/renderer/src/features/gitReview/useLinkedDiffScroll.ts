import { useCallback, useLayoutEffect, useRef } from 'react'
import type { RefObject, UIEventHandler } from 'react'

interface LinkedDiffScrollElements {
  leftCanvasRef: RefObject<HTMLDivElement | null>
  leftContentRef: RefObject<HTMLDivElement | null>
  leftPaneRef: RefObject<HTMLDivElement | null>
  rightCanvasRef: RefObject<HTMLDivElement | null>
  rightContentRef: RefObject<HTMLDivElement | null>
  rightPaneRef: RefObject<HTMLDivElement | null>
}

interface LinkedDiffScrollHandlers {
  onLeftScroll: UIEventHandler<HTMLDivElement>
  onRightScroll: UIEventHandler<HTMLDivElement>
}

/**
 * Links two native horizontal scrollports without putting high-frequency offsets in React state.
 * Both canvases are extended to the same overflow range so pixel-for-pixel synchronization does
 * not clamp early when one side contains shorter source lines.
 */
export function useLinkedDiffScroll({
  leftCanvasRef,
  leftContentRef,
  leftPaneRef,
  rightCanvasRef,
  rightContentRef,
  rightPaneRef
}: LinkedDiffScrollElements): LinkedDiffScrollHandlers {
  const measureFrameRef = useRef<number | null>(null)

  const measure = useCallback(() => {
    const leftCanvas = leftCanvasRef.current
    const leftContent = leftContentRef.current
    const leftPane = leftPaneRef.current
    const rightCanvas = rightCanvasRef.current
    const rightContent = rightContentRef.current
    const rightPane = rightPaneRef.current
    if (!leftCanvas || !leftContent || !leftPane || !rightCanvas || !rightContent || !rightPane) {
      return
    }

    const preservedScrollLeft = Math.max(leftPane.scrollLeft, rightPane.scrollLeft)

    // Remove the previous shared width before measuring intrinsic content; otherwise the old value
    // prevents the canvases from shrinking after a sidebar resize or a shorter patch replacement.
    leftCanvas.style.removeProperty('width')
    rightCanvas.style.removeProperty('width')

    const sharedOverflow = Math.max(
      leftContent.scrollWidth - leftPane.clientWidth,
      rightContent.scrollWidth - rightPane.clientWidth,
      0
    )
    leftCanvas.style.width = `${Math.ceil(leftPane.clientWidth + sharedOverflow)}px`
    rightCanvas.style.width = `${Math.ceil(rightPane.clientWidth + sharedOverflow)}px`

    const restoredScrollLeft = Math.min(preservedScrollLeft, sharedOverflow)
    leftPane.scrollLeft = restoredScrollLeft
    rightPane.scrollLeft = restoredScrollLeft
  }, [leftCanvasRef, leftContentRef, leftPaneRef, rightCanvasRef, rightContentRef, rightPaneRef])

  const scheduleMeasure = useCallback(() => {
    if (measureFrameRef.current !== null) cancelAnimationFrame(measureFrameRef.current)
    measureFrameRef.current = requestAnimationFrame(() => {
      measureFrameRef.current = null
      measure()
    })
  }, [measure])

  useLayoutEffect(() => {
    let disposed = false
    measure()

    const leftPane = leftPaneRef.current
    const leftContent = leftContentRef.current
    const rightPane = rightPaneRef.current
    const rightContent = rightContentRef.current
    const resizeObserver =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(scheduleMeasure)
    if (leftPane) resizeObserver?.observe(leftPane)
    if (rightPane) resizeObserver?.observe(rightPane)

    const mutationObserver =
      typeof MutationObserver === 'undefined' ? null : new MutationObserver(scheduleMeasure)
    const mutationOptions: MutationObserverInit = {
      characterData: true,
      childList: true,
      subtree: true
    }
    if (leftContent) mutationObserver?.observe(leftContent, mutationOptions)
    if (rightContent) mutationObserver?.observe(rightContent, mutationOptions)

    void document.fonts?.ready.then(() => {
      if (!disposed) scheduleMeasure()
    })

    return () => {
      disposed = true
      resizeObserver?.disconnect()
      mutationObserver?.disconnect()
      if (measureFrameRef.current !== null) cancelAnimationFrame(measureFrameRef.current)
    }
  }, [leftContentRef, leftPaneRef, measure, rightContentRef, rightPaneRef, scheduleMeasure])

  const synchronize = useCallback((source: HTMLDivElement, target: HTMLDivElement | null) => {
    // Programmatic scroll events naturally terminate because the source and target already match.
    // Unlike a frame-wide lock, this still accepts a genuine reverse-side gesture in the same frame.
    if (target && Math.abs(target.scrollLeft - source.scrollLeft) > 0.5) {
      target.scrollLeft = source.scrollLeft
    }
  }, [])

  return {
    onLeftScroll: useCallback(
      (event) => synchronize(event.currentTarget, rightPaneRef.current),
      [rightPaneRef, synchronize]
    ),
    onRightScroll: useCallback(
      (event) => synchronize(event.currentTarget, leftPaneRef.current),
      [leftPaneRef, synchronize]
    )
  }
}
