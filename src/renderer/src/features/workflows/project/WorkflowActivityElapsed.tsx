import { useLayoutEffect, useState } from 'react'
import type { WorkflowActivity } from '@mycopilot/protocol'
import { formatElapsedDuration } from '../../chat/components/chatMessageItemUtils'

/** Only this small label ticks; interval boundaries always come from the Host. */
export function WorkflowActivityElapsed({
  activity,
  language
}: {
  activity: WorkflowActivity | null | undefined
  language: string
}) {
  const [now, setNow] = useState(Date.now)
  const startedAt = activity?.startedAt
  const completedAt = activity?.completedAt
  useLayoutEffect(() => {
    if (startedAt === undefined || completedAt !== null) return
    let timer: ReturnType<typeof setInterval> | undefined
    const synchronize = () => {
      if (timer !== undefined) clearInterval(timer)
      timer = undefined
      if (document.visibilityState === 'hidden') return
      setNow(Date.now())
      timer = setInterval(() => setNow(Date.now()), 1_000)
    }
    synchronize()
    document.addEventListener('visibilitychange', synchronize)
    return () => {
      if (timer !== undefined) clearInterval(timer)
      document.removeEventListener('visibilitychange', synchronize)
    }
  }, [startedAt, completedAt])

  if (!activity) return null
  const duration = formatElapsedDuration((activity.completedAt ?? now) - activity.startedAt)
  return (
    <span className="project-workflows__elapsed">
      {language.startsWith('zh') ? '已连续运行 ' : 'Running for '}
      <span className="project-workflows__elapsed-duration">{duration}</span>
    </span>
  )
}
