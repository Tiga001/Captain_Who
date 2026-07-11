import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ToastContext, type ToastOptions } from './ToastContext'
import './ToastProvider.css'

interface ToastState {
  id: number
  message: string
  durationMs: number
}

const DEFAULT_TOAST_DURATION_MS = 1600

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toast, setToast] = useState<ToastState | null>(null)
  const nextToastIdRef = useRef(1)

  const showToast = useCallback((message: string, options: ToastOptions = {}) => {
    const normalizedMessage = message.trim()
    if (!normalizedMessage) return

    setToast({
      id: nextToastIdRef.current,
      message: normalizedMessage,
      durationMs: options.durationMs ?? DEFAULT_TOAST_DURATION_MS
    })
    nextToastIdRef.current += 1
  }, [])

  const value = useMemo(() => ({ showToast }), [showToast])

  useEffect(() => {
    if (!toast) return undefined
    const timeoutId = window.setTimeout(() => setToast(null), toast.durationMs)
    return () => window.clearTimeout(timeoutId)
  }, [toast])

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-viewport" aria-live="polite" aria-atomic="true">
        {toast && (
          <div className="toast-message" key={toast.id} role="status">
            {toast.message}
          </div>
        )}
      </div>
    </ToastContext.Provider>
  )
}
