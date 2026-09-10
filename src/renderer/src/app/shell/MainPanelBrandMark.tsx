import { useEffect, useRef, useState } from 'react'
import darkBrandMark from '../../../../../resources/brand-mark-dark.png'
import lightBrandMark from '../../../../../resources/brand-mark-light.png'

const WOBBLE_CLICKS_BEFORE_SAIL = 5
const SAIL_AWAY_MS = 640
const SAIL_PAUSE_MS = 1000
const SAIL_RETURN_MS = 760

function prefersReducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
}

function wobbleDurationMs(intervalMs: number) {
  if (!Number.isFinite(intervalMs) || intervalMs > 900) return 560
  return Math.max(160, 140 + intervalMs * 0.45)
}

export function MainPanelBrandMark() {
  const buttonRef = useRef<HTMLButtonElement>(null)
  const clickCountRef = useRef(0)
  const lastClickAtRef = useRef(0)
  const sailingRef = useRef(false)
  const tripGenerationRef = useRef(0)
  const wobbleAnimationRef = useRef<Animation | null>(null)
  const sailAnimationsRef = useRef<Animation[]>([])
  const pauseTimeoutRef = useRef<number | null>(null)
  const [motion, setMotion] = useState<'idle' | 'wobble' | 'sail'>('idle')

  useEffect(() => {
    return () => {
      tripGenerationRef.current += 1
      sailingRef.current = false
      wobbleAnimationRef.current?.cancel()
      for (const animation of sailAnimationsRef.current) animation.cancel()
      if (pauseTimeoutRef.current !== null) window.clearTimeout(pauseTimeoutRef.current)
    }
  }, [])

  const playWobble = (intervalMs: number) => {
    const node = buttonRef.current
    if (!node) return

    wobbleAnimationRef.current?.cancel()
    setMotion('wobble')
    const animation = node.animate(
      [
        { transform: 'rotate(0deg)' },
        { transform: 'rotate(-16deg)' },
        { transform: 'rotate(14deg)' },
        { transform: 'rotate(-8deg)' },
        { transform: 'rotate(0deg)' }
      ],
      { duration: wobbleDurationMs(intervalMs), easing: 'ease-in-out' }
    )
    wobbleAnimationRef.current = animation
    void animation.finished.then(
      () => {
        if (wobbleAnimationRef.current === animation && !sailingRef.current) setMotion('idle')
      },
      () => undefined
    )
  }

  const playSail = async () => {
    const node = buttonRef.current
    const slot = node?.parentElement
    if (!node || !slot) return

    wobbleAnimationRef.current?.cancel()
    wobbleAnimationRef.current = null
    sailingRef.current = true
    const generation = tripGenerationRef.current + 1
    tripGenerationRef.current = generation
    setMotion('sail')

    const slotBox = slot.getBoundingClientRect()
    const boatBox = node.getBoundingClientRect()
    const exitLeft = slotBox.left - boatBox.right - 4
    const enterRight = slotBox.right - boatBox.left + 4

    const leave = node.animate(
      [{ transform: 'translateX(0px)' }, { transform: `translateX(${exitLeft}px)` }],
      { duration: SAIL_AWAY_MS, easing: 'ease-in', fill: 'forwards' }
    )
    sailAnimationsRef.current = [leave]
    try {
      await leave.finished
    } catch {
      return
    }
    if (tripGenerationRef.current !== generation) return

    await new Promise<void>((resolve) => {
      pauseTimeoutRef.current = window.setTimeout(resolve, SAIL_PAUSE_MS)
    })
    pauseTimeoutRef.current = null
    if (tripGenerationRef.current !== generation) return

    leave.commitStyles()
    leave.cancel()
    node.style.transform = `translateX(${enterRight}px)`
    const enter = node.animate(
      [{ transform: `translateX(${enterRight}px)` }, { transform: 'translateX(0px)' }],
      { duration: SAIL_RETURN_MS, easing: 'ease-out', fill: 'forwards' }
    )
    sailAnimationsRef.current = [enter]
    try {
      await enter.finished
    } catch {
      return
    }
    if (tripGenerationRef.current !== generation) return

    enter.commitStyles()
    enter.cancel()
    node.style.removeProperty('transform')
    sailAnimationsRef.current = []
    sailingRef.current = false
    setMotion('idle')
  }

  const handleClick = () => {
    if (sailingRef.current || prefersReducedMotion()) return

    const now = performance.now()
    const interval = now - lastClickAtRef.current
    lastClickAtRef.current = now
    clickCountRef.current += 1

    if (clickCountRef.current < WOBBLE_CLICKS_BEFORE_SAIL) {
      playWobble(interval)
      return
    }

    clickCountRef.current = 0
    void playSail()
  }

  return (
    <div className="main-panel__title main-panel__title--brand">
      <button
        ref={buttonRef}
        className="main-panel__brand-mark-button"
        type="button"
        aria-label="Captain Who"
        data-motion={motion === 'idle' ? undefined : motion}
        onClick={handleClick}
      >
        <img
          className="main-panel__brand-mark main-panel__brand-mark--light"
          src={lightBrandMark}
          alt=""
        />
        <img
          className="main-panel__brand-mark main-panel__brand-mark--dark"
          src={darkBrandMark}
          alt=""
          aria-hidden="true"
        />
      </button>
    </div>
  )
}
