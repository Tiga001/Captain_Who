// The host owns update availability and download progress, independently of account access.
import { Download } from 'lucide-react'
import { useCallback, useEffect, useId, useRef, useState } from 'react'
import type { UpdateState } from '@mycopilot/host-api'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { hostClient } from '../../../host/hostClient'

interface LeftSidebarUpdateButtonProps {
  t: (key: TranslationKey) => string
}

export function LeftSidebarUpdateButton({ t }: LeftSidebarUpdateButtonProps) {
  const [state, setState] = useState<UpdateState | null>(null)
  const stateRef = useRef<UpdateState | null>(null)
  const mountedRef = useRef(false)
  const requestRef = useRef(false)
  const [requestPending, setRequestPending] = useState(false)
  const [requestError, setRequestError] = useState(false)
  const errorId = useId()
  const accept = useCallback((next: UpdateState) => {
    if (!mountedRef.current || next.revision <= (stateRef.current?.revision ?? -1)) return
    stateRef.current = next
    setState(next)
    setRequestError(false)
  }, [])

  useEffect(() => {
    // Older isolated browser fixtures may not provide the updates capability.
    const updates = hostClient.updates
    if (!updates) return
    mountedRef.current = true
    const unsubscribe = updates.onStateChanged(accept)
    void updates
      .getState()
      .then(accept)
      .catch(() => {
        // A failed initial read provides no evidence of an available update.
      })
    return () => {
      mountedRef.current = false
      unsubscribe()
    }
  }, [accept])

  const download = async () => {
    const current = stateRef.current
    if (
      requestRef.current ||
      !current?.version ||
      (current.status !== 'available' && current.status !== 'error')
    )
      return
    requestRef.current = true
    setRequestPending(true)
    setRequestError(false)
    const revision = current.revision
    try {
      accept(await hostClient.updates.download())
    } catch {
      if (mountedRef.current && stateRef.current?.revision === revision) setRequestError(true)
    } finally {
      requestRef.current = false
      if (mountedRef.current) setRequestPending(false)
    }
  }

  if (
    !state?.version ||
    !['available', 'downloading', 'installing', 'error'].includes(state.status)
  )
    return null

  const busy = state.status === 'downloading' || state.status === 'installing'
  // Display only host-reported progress; never advance it with a renderer timer.
  const percent = Math.floor(Math.min(100, Math.max(0, state.percent)))
  const label =
    state.status === 'installing'
      ? t('update.installing')
      : state.status === 'downloading'
        ? t('update.downloading').replace('{percent}', String(percent))
        : t('update.download')
  const error =
    state.status === 'error'
      ? (state.error ?? 'downloadFailed')
      : requestError && !busy
        ? 'downloadFailed'
        : null

  return (
    <div className="left-sidebar__update" data-status={state.status}>
      <button
        className="left-sidebar__update-button"
        data-variant={busy ? 'progress' : 'icon'}
        type="button"
        disabled={busy || requestPending}
        aria-label={label}
        aria-describedby={error ? errorId : undefined}
        title={busy ? undefined : label}
        onClick={() => {
          void download()
        }}
      >
        {busy ? <span>{label}</span> : <Download aria-hidden="true" />}
      </button>
      <span
        className="left-sidebar__update-announcement"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {busy ? label : ''}
      </span>
      {error && (
        <span className="left-sidebar__update-error" id={errorId} role="alert">
          {t(`update.error.${error}`)}
        </span>
      )}
    </div>
  )
}
