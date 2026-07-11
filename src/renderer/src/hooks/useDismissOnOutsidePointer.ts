import { useEffect } from 'react'
import type { RefObject } from 'react'

export function useDismissOnOutsidePointer<T extends HTMLElement>(
  ref: RefObject<T | null>,
  isOpen: boolean,
  onDismiss: () => void
) {
  useEffect(() => {
    if (!isOpen) return

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (ref.current?.contains(target)) return

      onDismiss()
    }

    document.addEventListener('pointerdown', handlePointerDown, true)
    return () => document.removeEventListener('pointerdown', handlePointerDown, true)
  }, [isOpen, onDismiss, ref])
}
