import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import './RollingLineCount.css'

const ROLL_DURATION_MS = 240

interface RollingLineCountProps {
  className: string
  sign: '+' | '-'
  value: number
}

/** One short transition at a time; intervening stream updates collapse to the latest value. */
export function RollingLineCount({ className, sign, value }: RollingLineCountProps) {
  const rootRef = useRef<HTMLSpanElement>(null)
  const latestValue = useRef(value)
  const [reducedMotion, setReducedMotion] = useState(
    () => window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
  const [frame, setFrame] = useState({ from: value, to: value, revision: 0 })
  const rolling = frame.from !== frame.to

  useLayoutEffect(() => {
    latestValue.current = value
    const root = rootRef.current
    const visible =
      root && root.getClientRects().length > 0 && getComputedStyle(root).visibility !== 'hidden'
    if (reducedMotion || !visible) {
      if (frame.from !== value || frame.to !== value) {
        setFrame({ from: value, to: value, revision: frame.revision + 1 })
      }
    } else if (!rolling && frame.to !== value) {
      setFrame({ from: frame.to, to: value, revision: frame.revision + 1 })
    }
  }, [value, reducedMotion, frame, rolling])

  useEffect(() => {
    if (!rolling) return
    const timer = window.setTimeout(() => {
      setFrame((current) => ({ ...current, from: current.to }))
    }, ROLL_DURATION_MS)
    return () => window.clearTimeout(timer)
  }, [rolling, frame.revision])

  useEffect(() => {
    const media = window.matchMedia('(prefers-reduced-motion: reduce)')
    const update = () => setReducedMotion(media.matches)
    media.addEventListener('change', update)
    // Expanding a retained disclosure shows current statistics, without replaying hidden edits.
    const disclosure = rootRef.current?.closest('details')
    const settle = () =>
      setFrame((current) => ({
        from: latestValue.current,
        to: latestValue.current,
        revision: current.revision + 1
      }))
    disclosure?.addEventListener('toggle', settle)
    return () => {
      media.removeEventListener('change', update)
      disclosure?.removeEventListener('toggle', settle)
    }
  }, [])

  const from = String(frame.from)
  const to = String(frame.to)
  const columns = Math.max(from.length, to.length)

  return (
    <span className={`${className} rolling-line-count`} ref={rootRef}>
      <span className="rolling-line-count__text">
        {sign}
        {value}
      </span>
      <span aria-hidden="true" className="rolling-line-count__visual">
        {sign}
        <span className="rolling-line-count__digits" style={{ width: `${to.length}ch` }}>
          {Array.from({ length: columns }, (_, place) => {
            const previous = from.at(-1 - place) ?? ''
            const next = to.at(-1 - place) ?? ''
            const changed = previous !== next
            return (
              <span
                className="rolling-line-count__column"
                key={place}
                style={{ right: `${place}ch` }}
              >
                {changed && previous ? (
                  <span className="rolling-line-count__out" key={`out-${frame.revision}`}>
                    {previous}
                  </span>
                ) : null}
                <span
                  className={changed ? 'rolling-line-count__in' : undefined}
                  key={`in-${frame.revision}`}
                >
                  {next}
                </span>
              </span>
            )
          })}
        </span>
      </span>
    </span>
  )
}
