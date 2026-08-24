import { useEffect } from 'react'
import type { RefObject } from 'react'

export function useDismissOnOutsidePointer<T extends HTMLElement>(
  ref: RefObject<T | null>,
  isOpen: boolean,
  onDismiss: () => void,
  ignoreTarget?: (target: Node) => boolean
) {
  useEffect(() => {
    if (!isOpen) return

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (ref.current?.contains(target)) return
      if (ignoreTarget?.(target)) return

      onDismiss()
    }

    document.addEventListener('pointerdown', handlePointerDown, true)
    return () => document.removeEventListener('pointerdown', handlePointerDown, true)
  }, [ignoreTarget, isOpen, onDismiss, ref])
}
