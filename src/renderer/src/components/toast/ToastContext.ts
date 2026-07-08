// Renderer UI.
import { createContext, useContext } from 'react'

export type ToastTone = 'accent'

export interface ToastOptions {
  durationMs?: number
  tone?: ToastTone
}

export interface ToastContextValue {
  showToast: (message: string, options?: ToastOptions) => void
}

export const ToastContext = createContext<ToastContextValue | null>(null)

export function useToast() {
  const context = useContext(ToastContext)
  if (!context) {
    throw new Error('useToast must be used within ToastProvider')
  }
  return context
}
